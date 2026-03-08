#!/usr/bin/env python3
"""
Compile LONG/SHORT(/REP) switching regions from Dorico MIDI using two signals:

1) Articulation-first classification from a VSL articulation dataset
   (MFT/OrchAPP articulations.json style with keyswitch output notes), then
2) note-length/IOI refinement for boundary recovery + fallback classification
   when articulation events are unavailable.

Outputs:
- CSV region map: start/end (sec + ticks), profile, source, reason
- Optional MIDI control track for DP bypass automation import

Example:
  python3 scripts/midi_profile_regions.py \
    --midi dorico_exports/V1.mid \
    --dataset "/Users/sk/MAXMSP Patches/Synchron_MFT_Master 2_WORK/articulations.json" \
    --output-csv qc_notes/V1_profile_regions.csv \
    --output-switch-mid qc_notes/V1_switch_control.mid
"""

from __future__ import annotations

import argparse
import csv
import json
import re
from dataclasses import dataclass
from pathlib import Path
from collections import defaultdict
from typing import Dict, Iterable, List, Optional, Sequence, Tuple

import mido


PROFILE_LONG = "LONG"
PROFILE_SHORT = "SHORT"
PROFILE_REP = "REP"


@dataclass
class NoteEvent:
    pitch: int
    channel: int
    velocity: int
    start_tick: int
    end_tick: int
    start_sec: float = 0.0
    end_sec: float = 0.0

    @property
    def duration_ms(self) -> float:
        return max(0.0, (self.end_sec - self.start_sec) * 1000.0)


@dataclass
class ArticulationEvent:
    tick: int
    sec: float
    profile: str
    reason: str


@dataclass
class Region:
    start_tick: int
    end_tick: int
    start_sec: float
    end_sec: float
    profile: str
    source: str
    reason: str


@dataclass
class ArticulationDataset:
    chord_to_profile: Dict[Tuple[int, ...], str]
    keyswitch_note_pool: set[int]


_NOTE_BASE = {
    "C": 0,
    "D": 2,
    "E": 4,
    "F": 5,
    "G": 7,
    "A": 9,
    "B": 11,
}


def note_name_to_midi(note: object) -> Optional[int]:
    """Match MFT conversion: midi=(octave+2)*12+semitone."""
    if isinstance(note, int):
        return note if 0 <= note <= 127 else None
    if not isinstance(note, str):
        return None

    m = re.match(r"^([A-Ga-g])([#b]?)(-?\d+)$", note.strip())
    if not m:
        return None

    step = m.group(1).upper()
    accidental = m.group(2)
    octave = int(m.group(3))

    semitone = _NOTE_BASE[step]
    if accidental == "#":
        semitone += 1
    elif accidental == "b":
        semitone -= 1

    midi = (octave + 2) * 12 + semitone
    if 0 <= midi <= 127:
        return midi
    return None


def normalize_label(s: str) -> str:
    return re.sub(r"\s+", " ", s.strip().lower())


def profile_from_articulation_name(name: str) -> str:
    """
    Normalize many naming variants from VSL/Dorico/MFT to LONG/SHORT/REP.
    This keeps mapping robust even when datasets contain old/new family labels.
    """
    n = normalize_label(name)

    if any(k in n for k in ["tremolo", "trill", "flutter", "repetition", "ostinato"]):
        return PROFILE_REP

    if any(
        k in n
        for k in [
            "short notes",
            "stacc",
            "spicc",
            "pizz",
            "detache",
            "snap",
            "col legno",
            "martelé",
            "martele",
        ]
    ):
        return PROFILE_SHORT

    if any(k in n for k in ["perf. legato", "per. leg.", "legato", "portamento"]):
        return PROFILE_LONG

    if any(k in n for k in ["long notes", "dynamics", "harmonics", "pont", "flautando"]):
        return PROFILE_LONG

    # Safe default for orchestral sustaining behavior.
    return PROFILE_LONG


def normalize_profile(profile: str, mode: str) -> str:
    if mode == "long_short" and profile == PROFILE_REP:
        return PROFILE_SHORT
    return profile


def load_articulation_dataset(dataset_path: Optional[Path], mode: str) -> ArticulationDataset:
    if dataset_path is None:
        return ArticulationDataset(chord_to_profile={}, keyswitch_note_pool=set())

    data = json.loads(dataset_path.read_text(encoding="utf-8"))
    arts = data.get("articulations", [])

    chord_to_profile: Dict[Tuple[int, ...], str] = {}
    ks_pool: set[int] = set()

    for art in arts:
        name = str(art.get("name", "")).strip()
        outputs = art.get("outputs", [])

        chord: List[int] = []
        for out in outputs:
            if not isinstance(out, dict):
                continue
            if str(out.get("type", "")).lower() != "noteon":
                continue
            midi = note_name_to_midi(out.get("pitch", out.get("note")))
            if midi is None:
                continue
            chord.append(midi)
            ks_pool.add(midi)

        if not chord:
            continue

        chord_key = tuple(sorted(set(chord)))
        profile = normalize_profile(profile_from_articulation_name(name), mode)

        # Resolve collisions deterministically: SHORT/REP outrank LONG.
        existing = chord_to_profile.get(chord_key)
        if existing is None:
            chord_to_profile[chord_key] = profile
        elif existing != profile:
            rank = {PROFILE_LONG: 0, PROFILE_SHORT: 1, PROFILE_REP: 2}
            chord_to_profile[chord_key] = profile if rank[profile] > rank[existing] else existing

    return ArticulationDataset(chord_to_profile=chord_to_profile, keyswitch_note_pool=ks_pool)


def build_tempo_map(mid: mido.MidiFile) -> List[Tuple[int, int]]:
    """Return tempo map as (tick, tempo_us_per_beat), sorted and deduped."""
    events: List[Tuple[int, int]] = [(0, 500000)]
    for track in mid.tracks:
        tick = 0
        for msg in track:
            tick += msg.time
            if msg.type == "set_tempo":
                events.append((tick, int(msg.tempo)))

    events.sort(key=lambda x: x[0])
    out: List[Tuple[int, int]] = []
    for t, tempo in events:
        if out and out[-1][0] == t:
            out[-1] = (t, tempo)
        else:
            out.append((t, tempo))
    return out


def tempo_at_tick(tick: int, tempo_map: Sequence[Tuple[int, int]]) -> int:
    tempo = tempo_map[0][1]
    for t, tm in tempo_map:
        if t > tick:
            break
        tempo = tm
    return tempo


def tick_to_sec(tick: int, tpq: int, tempo_map: Sequence[Tuple[int, int]]) -> float:
    sec = 0.0
    for i, (start_tick, tempo) in enumerate(tempo_map):
        end_tick = tempo_map[i + 1][0] if i + 1 < len(tempo_map) else tick
        if tick <= start_tick:
            break
        span_end = min(tick, end_tick)
        if span_end > start_tick:
            dticks = span_end - start_tick
            sec += (dticks / tpq) * (tempo / 1_000_000.0)
        if span_end == tick:
            break
    return sec


def ms_to_ticks(ms: float, tick_ref: int, tpq: int, tempo_map: Sequence[Tuple[int, int]]) -> int:
    tempo = tempo_at_tick(tick_ref, tempo_map)
    ticks = int(round((ms / 1000.0) * tpq * (1_000_000.0 / tempo)))
    return max(0, ticks)


def collect_tempo_events(mid: mido.MidiFile) -> List[Tuple[int, mido.MetaMessage]]:
    events: List[Tuple[int, mido.MetaMessage]] = []
    for track in mid.tracks:
        tick = 0
        for msg in track:
            tick += msg.time
            if msg.type == "set_tempo":
                events.append((tick, msg.copy(time=0)))
    if not events:
        events = [(0, mido.MetaMessage("set_tempo", tempo=500000, time=0))]

    # last tempo wins on same tick
    dedup: Dict[int, mido.MetaMessage] = {}
    for t, msg in events:
        dedup[t] = msg
    return sorted(dedup.items(), key=lambda x: x[0])


def extract_note_events(mid: mido.MidiFile, track_index: int, tempo_map: Sequence[Tuple[int, int]]) -> List[NoteEvent]:
    if track_index < 0 or track_index >= len(mid.tracks):
        raise ValueError(f"Track index {track_index} out of range (0..{len(mid.tracks)-1})")

    track = mid.tracks[track_index]
    tick = 0
    active: Dict[Tuple[int, int], List[Tuple[int, int]]] = {}
    out: List[NoteEvent] = []

    for msg in track:
        tick += msg.time
        if msg.type == "note_on" and msg.velocity > 0:
            key = (msg.note, msg.channel)
            active.setdefault(key, []).append((tick, msg.velocity))
        elif msg.type in ("note_off", "note_on") and getattr(msg, "velocity", 0) == 0:
            key = (msg.note, msg.channel)
            stack = active.get(key)
            if stack:
                start_tick, vel = stack.pop(0)
                out.append(
                    NoteEvent(
                        pitch=int(msg.note),
                        channel=int(msg.channel) + 1,
                        velocity=int(vel),
                        start_tick=start_tick,
                        end_tick=tick,
                    )
                )

    out.sort(key=lambda n: (n.start_tick, n.pitch))
    tpq = mid.ticks_per_beat
    for n in out:
        n.start_sec = tick_to_sec(n.start_tick, tpq, tempo_map)
        n.end_sec = tick_to_sec(n.end_tick, tpq, tempo_map)
    return out


def is_keyswitch_candidate(
    note: NoteEvent,
    keyswitch_pool: set[int],
    ks_vel_max: int,
    ks_dur_max_ms: float,
) -> bool:
    if not keyswitch_pool:
        return False
    return note.pitch in keyswitch_pool and note.velocity <= ks_vel_max and note.duration_ms <= ks_dur_max_ms


def match_chord_to_profile(chord: Sequence[int], chord_to_profile: Dict[Tuple[int, ...], str]) -> Optional[str]:
    if not chord_to_profile:
        return None

    chord_key = tuple(sorted(set(chord)))
    if chord_key in chord_to_profile:
        return chord_to_profile[chord_key]

    chord_set = set(chord_key)
    best_key: Optional[Tuple[int, ...]] = None
    for key in chord_to_profile.keys():
        key_set = set(key)
        if key_set.issubset(chord_set):
            if best_key is None or len(key) > len(best_key):
                best_key = key
    if best_key is not None:
        return chord_to_profile[best_key]
    return None


def detect_articulation_events_trigger(
    note_events: Sequence[NoteEvent],
    dataset: ArticulationDataset,
    ks_vel_max: int,
    ks_dur_max_ms: float,
    mode: str,
) -> Tuple[List[ArticulationEvent], set[int]]:
    candidates: Dict[int, List[NoteEvent]] = {}
    ks_note_starts: set[int] = set()

    for n in note_events:
        if is_keyswitch_candidate(n, dataset.keyswitch_note_pool, ks_vel_max, ks_dur_max_ms):
            candidates.setdefault(n.start_tick, []).append(n)
            ks_note_starts.add(id(n))

    events: List[ArticulationEvent] = []
    for tick in sorted(candidates.keys()):
        group = candidates[tick]
        chord = [n.pitch for n in group]
        profile = match_chord_to_profile(chord, dataset.chord_to_profile)
        if profile is None:
            continue
        profile = normalize_profile(profile, mode)
        sec = group[0].start_sec
        events.append(ArticulationEvent(tick=tick, sec=sec, profile=profile, reason=f"ks_chord={sorted(set(chord))}"))

    return events, ks_note_starts


def detect_articulation_events_latch(
    note_events: Sequence[NoteEvent],
    dataset: ArticulationDataset,
    mode: str,
    tpq: int,
    tempo_map: Sequence[Tuple[int, int]],
) -> Tuple[List[ArticulationEvent], set[int]]:
    """
    Detect articulation changes when keyswitches are exported as held/latching notes.
    Common in Dorico+VSL exports: KS notes are high velocity and long duration.
    """
    ks_ids: set[int] = set()
    on_by_tick: Dict[int, List[int]] = defaultdict(list)
    off_by_tick: Dict[int, List[int]] = defaultdict(list)

    for n in note_events:
        if n.pitch in dataset.keyswitch_note_pool:
            ks_ids.add(id(n))
            on_by_tick[n.start_tick].append(n.pitch)
            off_by_tick[n.end_tick].append(n.pitch)

    if not ks_ids:
        return [], ks_ids

    ticks = sorted(set(on_by_tick.keys()) | set(off_by_tick.keys()))
    active: set[int] = set()
    events: List[ArticulationEvent] = []
    last_profile: Optional[str] = None

    for tick in ticks:
        # Off first, then on: mirrors typical articulation switch replacement.
        for p in off_by_tick.get(tick, []):
            active.discard(p)
        for p in on_by_tick.get(tick, []):
            active.add(p)

        if not active:
            continue

        profile = match_chord_to_profile(sorted(active), dataset.chord_to_profile)
        if profile is None:
            continue
        profile = normalize_profile(profile, mode)

        if profile != last_profile:
            sec = tick_to_sec(tick, tpq, tempo_map)
            reason = f"ks_latch={sorted(active)}"
            events.append(ArticulationEvent(tick=tick, sec=sec, profile=profile, reason=reason))
            last_profile = profile

    return events, ks_ids


def detect_articulation_events(
    note_events: Sequence[NoteEvent],
    dataset: ArticulationDataset,
    ks_vel_max: int,
    ks_dur_max_ms: float,
    mode: str,
    ks_detection: str,
    tpq: int,
    tempo_map: Sequence[Tuple[int, int]],
) -> Tuple[List[ArticulationEvent], set[int], str]:
    """
    Return articulation events + keyswitch-note IDs + effective detection mode.
    """
    if ks_detection == "trigger":
        ev, ks = detect_articulation_events_trigger(
            note_events, dataset, ks_vel_max=ks_vel_max, ks_dur_max_ms=ks_dur_max_ms, mode=mode
        )
        return ev, ks, "trigger"

    if ks_detection == "latch":
        ev, ks = detect_articulation_events_latch(note_events, dataset, mode=mode, tpq=tpq, tempo_map=tempo_map)
        return ev, ks, "latch"

    # auto: prefer latch for Dorico/VSL exports, fallback to trigger.
    ev_latch, ks_latch = detect_articulation_events_latch(
        note_events, dataset, mode=mode, tpq=tpq, tempo_map=tempo_map
    )
    if ev_latch:
        return ev_latch, ks_latch, "latch(auto)"

    ev_trig, ks_trig = detect_articulation_events_trigger(
        note_events, dataset, ks_vel_max=ks_vel_max, ks_dur_max_ms=ks_dur_max_ms, mode=mode
    )
    return ev_trig, ks_trig, "trigger(auto)"


def fallback_profile_for_note(
    notes: Sequence[NoteEvent],
    idx: int,
    short_ms: float,
    long_ms: float,
    rep_ioi_ms: float,
    mode: str,
) -> Tuple[str, str]:
    n = notes[idx]
    dur_ms = n.duration_ms

    next_start = notes[idx + 1].start_sec if idx + 1 < len(notes) else None
    ioi_ms = ((next_start - n.start_sec) * 1000.0) if next_start is not None else None

    profile = PROFILE_LONG
    reason = f"dur={dur_ms:.1f}ms"

    if ioi_ms is not None and ioi_ms <= rep_ioi_ms:
        interval = abs(notes[idx + 1].pitch - n.pitch)
        if interval <= 2:
            profile = PROFILE_REP
            reason = f"ioi={ioi_ms:.1f}ms interval={interval}"

    if profile != PROFILE_REP:
        if dur_ms <= short_ms:
            profile = PROFILE_SHORT
            reason = f"dur={dur_ms:.1f}ms <= {short_ms:.1f}ms"
        elif dur_ms >= long_ms:
            profile = PROFILE_LONG
            reason = f"dur={dur_ms:.1f}ms >= {long_ms:.1f}ms"
        else:
            profile = PROFILE_LONG
            reason = f"dur={dur_ms:.1f}ms mid-band -> LONG"

    return normalize_profile(profile, mode), reason


def classify_regions(
    note_events: Sequence[NoteEvent],
    articulation_events: Sequence[ArticulationEvent],
    ks_note_ids: set[int],
    short_ms: float,
    long_ms: float,
    rep_ioi_ms: float,
    mode: str,
    merge_gap_ms: float,
    ks_duration_refine: bool,
    ks_refine_boundary_ms: float,
    ks_refine_short_max_ms: float,
    ks_refine_long_min_ms: float,
    ks_lookahead_ms: float,
    ks_lookbehind_ms: float,
    ks_texture_override: bool,
    texture_confirm_long: int,
    texture_confirm_short: int,
    texture_long_min_ms: float,
    texture_short_max_ms: float,
    texture_legato_gap_max_ms: float,
    texture_legato_min_note_ms: float,
    texture_short_gap_min_ms: float,
    texture_fast_long_ms: float,
    texture_fast_long_window_notes: int,
    texture_fast_long_max_gap_ms: float,
) -> List[Region]:
    music_notes = [n for n in note_events if id(n) not in ks_note_ids]
    if not music_notes:
        return []

    art_idx = 0
    active_profile: Optional[str] = None
    active_reason = ""
    regions: List[Region] = []
    last_active_profile: Optional[str] = None
    texture_latch: Optional[str] = None
    long_streak = 0
    short_streak = 0

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

        fallback_profile, fb_reason = fallback_profile_for_note(
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

            # 1) Look-ahead boundary recovery:
            #    if the next KS change is extremely close and note-duration
            #    fallback agrees with that next profile, switch early.
            if next_event is not None and fallback_profile == next_event.profile:
                dt_next_ms = (next_event.sec - n.start_sec) * 1000.0
                if 0.0 <= dt_next_ms <= ks_lookahead_ms:
                    profile = next_event.profile
                    source = "articulation_lookahead"
                    reason = f"{next_event.reason}; lookahead={dt_next_ms:.1f}ms; {fb_reason}"

            # 2) Look-behind boundary recovery:
            #    if a KS change arrived slightly early, keep previous profile
            #    for the first note after the switch when duration evidence agrees.
            if source == "articulation" and current_event is not None and prev_event is not None:
                if fallback_profile == prev_event.profile and prev_event.profile != current_event.profile:
                    dt_prev_ms = (n.start_sec - current_event.sec) * 1000.0
                    if 0.0 <= dt_prev_ms <= ks_lookbehind_ms:
                        profile = prev_event.profile
                        source = "articulation_lookbehind"
                        reason = f"{prev_event.reason}; lookbehind={dt_prev_ms:.1f}ms; {fb_reason}"

            # 3) KS-first, duration-second refinement near articulation boundaries.
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
                    if fallback_profile == PROFILE_SHORT and n.duration_ms <= ks_refine_short_max_ms:
                        should_refine = True
                    elif fallback_profile == PROFILE_LONG and n.duration_ms >= ks_refine_long_min_ms:
                        should_refine = True
                    elif fallback_profile == PROFILE_REP and mode == "long_short_rep":
                        should_refine = True

                    if should_refine:
                        profile = fallback_profile
                        source = "articulation+duration"
                        reason = (
                            f"{active_reason}; boundary={nearest_boundary_ms:.1f}ms; "
                            f"dur_refine={fb_reason}"
                        )

            # 4) Texture latch override:
            #    if KS profile appears stale for several notes, let note-length
            #    evidence temporarily drive the profile until KS updates again.
            if (
                ks_texture_override
                and source == "articulation"
                and active_profile in {PROFILE_LONG, PROFILE_SHORT}
                and fallback_profile in {PROFILE_LONG, PROFILE_SHORT}
            ):
                if art_changed:
                    texture_latch = None
                    long_streak = 0
                    short_streak = 0

                current = texture_latch if texture_latch is not None else active_profile

                if current == PROFILE_SHORT:
                    long_evidence = False
                    if fallback_profile == PROFILE_LONG and n.duration_ms >= texture_long_min_ms:
                        long_evidence = True
                    # Connected-note evidence for legato lines where per-note duration
                    # alone would otherwise look "short".
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
                        texture_latch = PROFILE_LONG
                        current = PROFILE_LONG
                        long_streak = 0
                        short_streak = 0
                else:
                    short_evidence = (
                        fallback_profile == PROFILE_SHORT
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
                        texture_latch = PROFILE_SHORT
                        current = PROFILE_SHORT
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

        regions.append(
            Region(
                start_tick=n.start_tick,
                end_tick=n.end_tick,
                start_sec=n.start_sec,
                end_sec=n.end_sec,
                profile=profile,
                source=source,
                reason=reason,
            )
        )

    return merge_regions(regions, merge_gap_ms=merge_gap_ms)


def merge_regions(regions: Sequence[Region], merge_gap_ms: float) -> List[Region]:
    if not regions:
        return []

    merged = [regions[0]]
    max_gap_sec = merge_gap_ms / 1000.0

    for r in regions[1:]:
        prev = merged[-1]
        gap = r.start_sec - prev.end_sec
        if r.profile == prev.profile and gap <= max_gap_sec:
            prev.end_tick = max(prev.end_tick, r.end_tick)
            prev.end_sec = max(prev.end_sec, r.end_sec)
            prev.reason = f"{prev.reason}; merged"
            if prev.source != r.source:
                prev.source = "mixed"
        else:
            merged.append(r)
    return merged


def write_regions_csv(path: Path, regions: Sequence[Region]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", newline="", encoding="utf-8") as f:
        w = csv.writer(f)
        w.writerow(["start_sec", "end_sec", "start_tick", "end_tick", "profile", "source", "reason"])
        for r in regions:
            w.writerow(
                [
                    f"{r.start_sec:.4f}",
                    f"{r.end_sec:.4f}",
                    r.start_tick,
                    r.end_tick,
                    r.profile,
                    r.source,
                    r.reason,
                ]
            )


def build_profile_state(profile: str, mode: str) -> Dict[str, int]:
    if mode == "long_short":
        if profile == PROFILE_SHORT:
            return {PROFILE_LONG: 127, PROFILE_SHORT: 0}
        return {PROFILE_LONG: 0, PROFILE_SHORT: 127}

    if profile == PROFILE_SHORT:
        return {PROFILE_LONG: 127, PROFILE_SHORT: 0, PROFILE_REP: 127}
    if profile == PROFILE_REP:
        return {PROFILE_LONG: 127, PROFILE_SHORT: 127, PROFILE_REP: 0}
    return {PROFILE_LONG: 0, PROFILE_SHORT: 127, PROFILE_REP: 127}


def write_switch_midi(
    path: Path,
    source_mid: mido.MidiFile,
    tempo_events: Sequence[Tuple[int, mido.MetaMessage]],
    regions: Sequence[Region],
    tempo_map: Sequence[Tuple[int, int]],
    overlap_ms: float,
    pre_roll_ms: float,
    mode: str,
    channel: int,
    cc_long: int,
    cc_short: int,
    cc_rep: int,
) -> None:
    if not regions:
        return

    cc_map = {PROFILE_LONG: cc_long, PROFILE_SHORT: cc_short, PROFILE_REP: cc_rep}

    events: List[Tuple[int, int, mido.Message]] = []

    first_profile = regions[0].profile
    state = build_profile_state(first_profile, mode)
    for p, val in state.items():
        if p == PROFILE_REP and mode == "long_short":
            continue
        events.append(
            (0, 0, mido.Message("control_change", channel=channel - 1, control=cc_map[p], value=val, time=0))
        )

    for i in range(1, len(regions)):
        prev = regions[i - 1]
        cur = regions[i]
        if cur.profile == prev.profile:
            continue

        t = cur.start_tick
        overlap_ticks = ms_to_ticks(overlap_ms, t, source_mid.ticks_per_beat, tempo_map)
        pre_roll_ticks = ms_to_ticks(pre_roll_ms, t, source_mid.ticks_per_beat, tempo_map)
        t_on = max(0, t - pre_roll_ticks)

        prev_state = build_profile_state(prev.profile, mode)
        cur_state = build_profile_state(cur.profile, mode)

        # Activate new profile a little before boundary for reliable first-note capture.
        for p in cur_state.keys():
            if p == PROFILE_REP and mode == "long_short":
                continue
            if cur_state[p] != prev_state.get(p):
                if cur_state[p] == 0:
                    events.append(
                        (
                            t_on,
                            0,
                            mido.Message("control_change", channel=channel - 1, control=cc_map[p], value=0, time=0),
                        )
                    )

        # Deactivate old profile after overlap.
        t2 = t + overlap_ticks
        for p in prev_state.keys():
            if p == PROFILE_REP and mode == "long_short":
                continue
            if cur_state.get(p) != prev_state[p] and cur_state.get(p, 127) == 127:
                events.append(
                    (
                        t2,
                        1,
                        mido.Message("control_change", channel=channel - 1, control=cc_map[p], value=127, time=0),
                    )
                )

    events.sort(key=lambda x: (x[0], x[1]))

    out_mid = mido.MidiFile(ticks_per_beat=source_mid.ticks_per_beat)

    tempo_track = mido.MidiTrack()
    out_mid.tracks.append(tempo_track)
    last_tick = 0
    for tick, meta in tempo_events:
        delta = max(0, tick - last_tick)
        tempo_track.append(meta.copy(time=delta))
        last_tick = tick
    tempo_track.append(mido.MetaMessage("end_of_track", time=0))

    ctrl_track = mido.MidiTrack()
    out_mid.tracks.append(ctrl_track)
    last_tick = 0
    for tick, _, msg in events:
        delta = max(0, tick - last_tick)
        ctrl_track.append(msg.copy(time=delta))
        last_tick = tick
    ctrl_track.append(mido.MetaMessage("end_of_track", time=0))

    path.parent.mkdir(parents=True, exist_ok=True)
    out_mid.save(path)


def main() -> int:
    ap = argparse.ArgumentParser(description="Compile LONG/SHORT(/REP) switching regions from MIDI")
    ap.add_argument("--midi", required=True, type=Path, help="Input MIDI file")
    ap.add_argument("--track-index", type=int, default=0, help="Track index containing instrument notes + keyswitches")
    ap.add_argument(
        "--dataset",
        type=Path,
        default=Path("/Users/sk/MAXMSP Patches/Synchron_MFT_Master 2_WORK/articulations.json"),
        help="Path to articulation dataset JSON (MFT/OrchAPP format)",
    )
    ap.add_argument(
        "--mode",
        choices=["long_short", "long_short_rep"],
        default="long_short",
        help="Profile output mode (default: long_short)",
    )
    ap.add_argument("--long-ms", type=float, default=300.0, help="LONG fallback threshold")
    ap.add_argument("--short-ms", type=float, default=220.0, help="SHORT fallback threshold")
    ap.add_argument("--rep-ioi-ms", type=float, default=260.0, help="REP fallback IOI threshold")
    ap.add_argument("--ks-vel-max", type=int, default=4, help="Max velocity considered keyswitch")
    ap.add_argument("--ks-dur-max-ms", type=float, default=250.0, help="Max duration considered keyswitch")
    ap.add_argument(
        "--ks-detection",
        choices=["auto", "latch", "trigger"],
        default="auto",
        help="Keyswitch detection strategy (default: auto, prefers latch then trigger)",
    )
    ap.add_argument("--merge-gap-ms", type=float, default=30.0, help="Merge same-profile regions across small gaps")
    ap.add_argument("--overlap-ms", type=float, default=35.0, help="Switch overlap for control MIDI")
    ap.add_argument(
        "--switch-pre-roll-ms",
        type=float,
        default=25.0,
        help="Activate the incoming profile this many ms before boundary",
    )
    ap.add_argument(
        "--no-ks-duration-refine",
        action="store_true",
        help="Disable duration/IOI refinement when an articulation profile is active",
    )
    ap.add_argument(
        "--ks-refine-boundary-ms",
        type=float,
        default=120.0,
        help="Only apply KS+duration refinement near an articulation boundary",
    )
    ap.add_argument(
        "--ks-refine-short-max-ms",
        type=float,
        default=220.0,
        help="Allow LONG->SHORT refinement only when note duration <= this",
    )
    ap.add_argument(
        "--ks-refine-long-min-ms",
        type=float,
        default=420.0,
        help="Allow SHORT->LONG refinement only when note duration >= this",
    )
    ap.add_argument(
        "--ks-lookahead-ms",
        type=float,
        default=90.0,
        help="Boundary recovery: adopt next KS profile when very close to boundary",
    )
    ap.add_argument(
        "--ks-lookbehind-ms",
        type=float,
        default=70.0,
        help="Boundary recovery: keep previous KS profile for first note after an early switch",
    )
    ap.add_argument(
        "--no-ks-texture-override",
        action="store_true",
        help="Disable KS-segment texture latch override",
    )
    ap.add_argument(
        "--texture-confirm-long",
        type=int,
        default=3,
        help="Consecutive LONG-evidence notes needed to override stale SHORT KS",
    )
    ap.add_argument(
        "--texture-confirm-short",
        type=int,
        default=3,
        help="Consecutive SHORT-evidence notes needed to override stale LONG KS",
    )
    ap.add_argument(
        "--texture-long-min-ms",
        type=float,
        default=300.0,
        help="Min duration for LONG evidence inside texture override",
    )
    ap.add_argument(
        "--texture-short-max-ms",
        type=float,
        default=220.0,
        help="Max duration for SHORT evidence inside texture override",
    )
    ap.add_argument(
        "--texture-legato-gap-max-ms",
        type=float,
        default=18.0,
        help="Max next-note gap considered connected/legato evidence for LONG override",
    )
    ap.add_argument(
        "--texture-legato-min-note-ms",
        type=float,
        default=120.0,
        help="Min note duration to count connected-note LONG evidence",
    )
    ap.add_argument(
        "--texture-short-gap-min-ms",
        type=float,
        default=20.0,
        help="Min next-note gap to count SHORT evidence against LONG",
    )
    ap.add_argument(
        "--texture-fast-long-ms",
        type=float,
        default=430.0,
        help="Immediate LONG trigger duration inside SHORT KS when note is clearly sustained and connected",
    )
    ap.add_argument(
        "--texture-fast-long-window-notes",
        type=int,
        default=4,
        help="Connected-note window size for fast LONG trigger",
    )
    ap.add_argument(
        "--texture-fast-long-max-gap-ms",
        type=float,
        default=20.0,
        help="Max per-step gap allowed inside fast LONG connected-note window",
    )

    ap.add_argument("--output-csv", required=True, type=Path, help="Output CSV region map")
    ap.add_argument(
        "--output-note-audit-csv",
        type=Path,
        help="Optional per-note audit CSV (no merge, highest granularity)",
    )
    ap.add_argument("--output-switch-mid", type=Path, help="Optional output MIDI for bypass CC switching")
    ap.add_argument("--switch-channel", type=int, default=1, help="MIDI channel for switch CCs (1-16)")
    ap.add_argument("--cc-long", type=int, default=90, help="CC number for LONG bypass control")
    ap.add_argument("--cc-short", type=int, default=91, help="CC number for SHORT bypass control")
    ap.add_argument("--cc-rep", type=int, default=92, help="CC number for REP bypass control")

    args = ap.parse_args()

    mid = mido.MidiFile(args.midi)
    tempo_map = build_tempo_map(mid)
    tempo_events = collect_tempo_events(mid)

    dataset_path = args.dataset if args.dataset and args.dataset.exists() else None
    dataset = load_articulation_dataset(dataset_path, mode=args.mode)

    note_events = extract_note_events(mid, track_index=args.track_index, tempo_map=tempo_map)
    articulation_events, ks_note_ids, detection_used = detect_articulation_events(
        note_events,
        dataset,
        ks_vel_max=args.ks_vel_max,
        ks_dur_max_ms=args.ks_dur_max_ms,
        mode=args.mode,
        ks_detection=args.ks_detection,
        tpq=mid.ticks_per_beat,
        tempo_map=tempo_map,
    )

    regions = classify_regions(
        note_events,
        articulation_events,
        ks_note_ids,
        short_ms=args.short_ms,
        long_ms=args.long_ms,
        rep_ioi_ms=args.rep_ioi_ms,
        mode=args.mode,
        merge_gap_ms=args.merge_gap_ms,
        ks_duration_refine=not args.no_ks_duration_refine,
        ks_refine_boundary_ms=max(0.0, args.ks_refine_boundary_ms),
        ks_refine_short_max_ms=max(0.0, args.ks_refine_short_max_ms),
        ks_refine_long_min_ms=max(0.0, args.ks_refine_long_min_ms),
        ks_lookahead_ms=max(0.0, args.ks_lookahead_ms),
        ks_lookbehind_ms=max(0.0, args.ks_lookbehind_ms),
        ks_texture_override=not args.no_ks_texture_override,
        texture_confirm_long=max(1, args.texture_confirm_long),
        texture_confirm_short=max(1, args.texture_confirm_short),
        texture_long_min_ms=max(0.0, args.texture_long_min_ms),
        texture_short_max_ms=max(0.0, args.texture_short_max_ms),
        texture_legato_gap_max_ms=args.texture_legato_gap_max_ms,
        texture_legato_min_note_ms=args.texture_legato_min_note_ms,
        texture_short_gap_min_ms=args.texture_short_gap_min_ms,
        texture_fast_long_ms=args.texture_fast_long_ms,
        texture_fast_long_window_notes=max(2, args.texture_fast_long_window_notes),
        texture_fast_long_max_gap_ms=args.texture_fast_long_max_gap_ms,
    )

    write_regions_csv(args.output_csv, regions)
    print(f"Wrote {len(regions)} regions -> {args.output_csv}")
    print(f"Dataset chords loaded: {len(dataset.chord_to_profile)} | KS pool size: {len(dataset.keyswitch_note_pool)}")
    print(f"Detected articulation events: {len(articulation_events)} (mode={detection_used})")

    if args.output_note_audit_csv:
        note_level_regions = classify_regions(
            note_events,
            articulation_events,
            ks_note_ids,
            short_ms=args.short_ms,
            long_ms=args.long_ms,
            rep_ioi_ms=args.rep_ioi_ms,
            mode=args.mode,
            merge_gap_ms=-1.0,
            ks_duration_refine=not args.no_ks_duration_refine,
            ks_refine_boundary_ms=max(0.0, args.ks_refine_boundary_ms),
            ks_refine_short_max_ms=max(0.0, args.ks_refine_short_max_ms),
            ks_refine_long_min_ms=max(0.0, args.ks_refine_long_min_ms),
            ks_lookahead_ms=max(0.0, args.ks_lookahead_ms),
            ks_lookbehind_ms=max(0.0, args.ks_lookbehind_ms),
            ks_texture_override=not args.no_ks_texture_override,
            texture_confirm_long=max(1, args.texture_confirm_long),
            texture_confirm_short=max(1, args.texture_confirm_short),
            texture_long_min_ms=max(0.0, args.texture_long_min_ms),
            texture_short_max_ms=max(0.0, args.texture_short_max_ms),
            texture_legato_gap_max_ms=args.texture_legato_gap_max_ms,
            texture_legato_min_note_ms=args.texture_legato_min_note_ms,
            texture_short_gap_min_ms=args.texture_short_gap_min_ms,
            texture_fast_long_ms=args.texture_fast_long_ms,
            texture_fast_long_window_notes=max(2, args.texture_fast_long_window_notes),
            texture_fast_long_max_gap_ms=args.texture_fast_long_max_gap_ms,
        )
        write_regions_csv(args.output_note_audit_csv, note_level_regions)
        print(f"Wrote {len(note_level_regions)} note-level regions -> {args.output_note_audit_csv}")

    if args.output_switch_mid:
        write_switch_midi(
            path=args.output_switch_mid,
            source_mid=mid,
            tempo_events=tempo_events,
            regions=regions,
            tempo_map=tempo_map,
            overlap_ms=args.overlap_ms,
            pre_roll_ms=max(0.0, args.switch_pre_roll_ms),
            mode=args.mode,
            channel=max(1, min(16, args.switch_channel)),
            cc_long=max(0, min(127, args.cc_long)),
            cc_short=max(0, min(127, args.cc_short)),
            cc_rep=max(0, min(127, args.cc_rep)),
        )
        print(f"Wrote switch MIDI -> {args.output_switch_mid}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
