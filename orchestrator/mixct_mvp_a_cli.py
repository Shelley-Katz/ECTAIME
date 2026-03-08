#!/usr/bin/env python3
"""
MixCT MVP-A CLI (DP-first, offline)

Purpose:
1) Parse conductor-style text directives over bar ranges.
2) Resolve targets to orchestral mix buses using a session map.
3) Generate MIDI CC automation tracks (one per bus) for DP import.

Example:
  .venv/bin/python orchestrator/mixct_mvp_a_cli.py \
    --source-midi score_conversion_drop/outbox/Waghalter/Waghalter__DP_PREP_WAV_DENSE.mid \
    --session-map contracts/mixct_session_map.waghalter.yaml \
    --directives score_conversion_drop/outbox/Waghalter/mixct_directives.txt \
    --output-dir score_conversion_drop/outbox/Waghalter/mixct_mvp_a
"""

from __future__ import annotations

import argparse
import csv
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Iterable, List, Optional, Sequence, Tuple

import mido

try:
    import yaml  # type: ignore
except Exception:  # pragma: no cover
    yaml = None


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPTS_DIR = REPO_ROOT / "scripts"
if str(SCRIPTS_DIR) not in sys.path:
    sys.path.insert(0, str(SCRIPTS_DIR))

import midi_profile_regions as mpr  # noqa: E402


ROLE_CANON = {
    "PRIMARY": "PRIMARY",
    "LEAD": "PRIMARY",
    "COUNTERPOINT": "COUNTERPOINT",
    "SECONDARY": "SECONDARY",
    "SUPPORT": "SECONDARY",
    "ACCOMPANIMENT": "ACCOMPANIMENT",
    "ACCOMP": "ACCOMPANIMENT",
    "BACKGROUND": "ACCOMPANIMENT",
}


@dataclass
class BusSpec:
    id: str
    label: str
    cc: int
    channel_1based: int
    min_db: float
    max_db: float
    default_db: float
    aliases: List[str] = field(default_factory=list)
    include_track_patterns: List[str] = field(default_factory=list)
    include_track_names: List[str] = field(default_factory=list)


@dataclass
class Directive:
    line_no: int
    raw: str
    start_bar: int
    end_bar: int
    explicit: Dict[str, str]
    rest_role: Optional[str]


@dataclass
class GlobalDefaults:
    ramp_in_ms: float
    control_step_ms: float


def normalize_ws(s: str) -> str:
    return re.sub(r"\s+", " ", s.strip())


def normalize_text(s: str) -> str:
    return normalize_ws(re.sub(r"[^a-zA-Z0-9]+", " ", s).lower())


def track_name_of(track: mido.MidiTrack, fallback: str) -> str:
    for msg in track:
        if msg.is_meta and msg.type == "track_name":
            name = normalize_ws(str(getattr(msg, "name", "") or ""))
            if name:
                return name
    return fallback


def collect_global_meta_events(mid: mido.MidiFile) -> List[Tuple[int, int, mido.MetaMessage]]:
    keep_meta_types = {
        "set_tempo",
        "time_signature",
        "key_signature",
        "marker",
        "cue_marker",
        "text",
        "lyrics",
    }
    raw: List[Tuple[int, int, mido.MetaMessage]] = []
    order = 0
    for tr in mid.tracks:
        tick = 0
        for msg in tr:
            tick += int(msg.time)
            if not msg.is_meta or msg.type == "end_of_track":
                continue
            if msg.type not in keep_meta_types:
                continue
            raw.append((tick, order, msg.copy(time=0)))
            order += 1
    # last tempo wins at same tick
    tempo_by_tick: Dict[int, Tuple[int, mido.MetaMessage]] = {}
    others: List[Tuple[int, int, mido.MetaMessage]] = []
    for tick, idx, msg in raw:
        if msg.type == "set_tempo":
            tempo_by_tick[tick] = (idx, msg)
        else:
            others.append((tick, idx, msg))
    out: List[Tuple[int, int, mido.MetaMessage]] = []
    for tick, (idx, msg) in tempo_by_tick.items():
        out.append((tick, idx, msg))
    out.extend(others)
    if not any(msg.type == "set_tempo" for _, _, msg in out):
        out.append((0, 10_000_000, mido.MetaMessage("set_tempo", tempo=500000, time=0)))
    out.sort(key=lambda x: (x[0], x[1]))
    return out


def absolute_track_end_tick(track: mido.MidiTrack) -> int:
    tick = 0
    for msg in track:
        tick += int(msg.time)
    return tick


def build_time_signature_map(mid: mido.MidiFile) -> List[Tuple[int, int, int]]:
    events: List[Tuple[int, int, int]] = [(0, 4, 4)]
    for tr in mid.tracks:
        tick = 0
        for msg in tr:
            tick += int(msg.time)
            if msg.is_meta and msg.type == "time_signature":
                events.append((tick, int(msg.numerator), int(msg.denominator)))
    events.sort(key=lambda x: x[0])
    dedup: Dict[int, Tuple[int, int]] = {}
    for tick, num, den in events:
        dedup[tick] = (num, den)
    out = [(t, dedup[t][0], dedup[t][1]) for t in sorted(dedup.keys())]
    return out


def build_bar_starts(mid: mido.MidiFile, end_tick_hint: Optional[int] = None) -> List[int]:
    tpq = int(mid.ticks_per_beat)
    ts_map = build_time_signature_map(mid)
    max_tick = max(absolute_track_end_tick(t) for t in mid.tracks)
    if end_tick_hint is not None:
        max_tick = max(max_tick, int(end_tick_hint))
    # Extend one extra long bar to safely include terminal region.
    max_tick += tpq * 16

    starts: List[int] = [0]
    current_tick = 0
    bar_start = 0
    ts_idx = 0
    while bar_start <= max_tick:
        while ts_idx + 1 < len(ts_map) and bar_start >= ts_map[ts_idx + 1][0]:
            ts_idx += 1
        _, num, den = ts_map[ts_idx]
        ticks_per_beat = tpq * (4.0 / float(den))
        bar_len = max(1, int(round(float(num) * ticks_per_beat)))
        if bar_start > starts[-1]:
            starts.append(bar_start)
        bar_start += bar_len
        current_tick = bar_start
        if current_tick > max_tick and len(starts) > 2:
            break
    if starts[-1] <= max_tick:
        starts.append(max_tick + 1)
    else:
        starts.append(starts[-1] + tpq * 4)
    return starts


def bar_range_to_ticks(bar_starts: Sequence[int], start_bar: int, end_bar: int) -> Tuple[int, int]:
    sb = max(1, int(start_bar))
    eb = max(sb, int(end_bar))
    if eb + 1 >= len(bar_starts):
        raise ValueError(
            f"Directive refers to bar {eb}, but bar map currently has only {len(bar_starts)-1} bars."
        )
    start_tick = int(bar_starts[sb - 1])
    end_tick_excl = int(bar_starts[eb])
    return start_tick, end_tick_excl


def load_yaml(path: Path) -> Dict[str, Any]:
    if yaml is None:
        raise RuntimeError("PyYAML not installed in this environment.")
    data = yaml.safe_load(path.read_text(encoding="utf-8")) or {}
    if not isinstance(data, dict):
        raise ValueError(f"YAML root must be object: {path}")
    return data


def parse_session_map(path: Path) -> Tuple[List[BusSpec], Dict[str, str], Dict[str, float], GlobalDefaults]:
    data = load_yaml(path)
    buses_raw = data.get("buses", [])
    if not isinstance(buses_raw, list) or not buses_raw:
        raise ValueError("Session map must define non-empty 'buses' list.")
    role_offsets_raw = data.get("role_db_offsets", {})
    role_offsets: Dict[str, float] = {}
    for k, v in role_offsets_raw.items():
        ck = ROLE_CANON.get(str(k).upper())
        if ck:
            role_offsets[ck] = float(v)
    for required in ["PRIMARY", "COUNTERPOINT", "SECONDARY", "ACCOMPANIMENT"]:
        role_offsets.setdefault(required, 0.0 if required == "PRIMARY" else (-3.0 if required == "COUNTERPOINT" else (-6.0 if required == "SECONDARY" else -10.0)))

    defaults_raw = data.get("defaults", {})
    defaults = GlobalDefaults(
        ramp_in_ms=float(defaults_raw.get("ramp_in_ms", 220.0)),
        control_step_ms=float(defaults_raw.get("control_step_ms", 40.0)),
    )

    alias_to_bus: Dict[str, str] = {}
    buses: List[BusSpec] = []
    for b in buses_raw:
        if not isinstance(b, dict):
            continue
        bid = str(b.get("id", "")).strip()
        if not bid:
            continue
        spec = BusSpec(
            id=bid,
            label=str(b.get("label", bid)),
            cc=int(b.get("cc", 0)),
            channel_1based=int(b.get("channel", 16)),
            min_db=float(b.get("min_db", -18.0)),
            max_db=float(b.get("max_db", 6.0)),
            default_db=float(b.get("default_db", 0.0)),
            aliases=[str(x) for x in b.get("aliases", []) if str(x).strip()],
            include_track_patterns=[str(x) for x in b.get("include_track_patterns", []) if str(x).strip()],
            include_track_names=[str(x) for x in b.get("include_track_names", []) if str(x).strip()],
        )
        buses.append(spec)
        for a in [bid, spec.label] + spec.aliases:
            na = normalize_text(a)
            if na:
                alias_to_bus[na] = bid

    entity_aliases = data.get("entity_aliases", {})
    if isinstance(entity_aliases, dict):
        for alias, bus_id in entity_aliases.items():
            na = normalize_text(str(alias))
            if na and str(bus_id).strip():
                alias_to_bus[na] = str(bus_id).strip()

    return buses, alias_to_bus, role_offsets, defaults


def list_main_instrument_tracks(mid: mido.MidiFile) -> List[str]:
    out: List[str] = []
    for i, tr in enumerate(mid.tracks):
        name = track_name_of(tr, f"Track {i}")
        n = normalize_ws(name)
        if not n:
            continue
        if n.lower() == "ect conductor":
            continue
        if n.endswith(" KS") or n.endswith(" ArtMap"):
            continue
        # Keep only tracks with note messages.
        has_note = any((m.type == "note_on" and int(getattr(m, "velocity", 0)) > 0) or (m.type == "note_off") for m in tr)
        if has_note:
            out.append(n)
    return out


def assign_tracks_to_buses(track_names: Sequence[str], buses: Sequence[BusSpec]) -> Dict[str, List[str]]:
    by_bus: Dict[str, List[str]] = {b.id: [] for b in buses}
    for t in track_names:
        assigned = False
        for b in buses:
            if any(normalize_text(t) == normalize_text(x) for x in b.include_track_names):
                by_bus[b.id].append(t)
                assigned = True
                break
            if any(re.search(pat, t, flags=re.IGNORECASE) for pat in b.include_track_patterns):
                by_bus[b.id].append(t)
                assigned = True
                break
        if not assigned:
            # intentionally unassigned tracks are allowed; they can be manually mapped later.
            pass
    return by_bus


def normalize_role(word: str) -> Optional[str]:
    return ROLE_CANON.get(str(word).upper().strip())


def split_targets(raw_targets: str) -> List[str]:
    t = normalize_ws(raw_targets)
    t = re.sub(r"\b(the|all|of|and)\b", " ", t, flags=re.IGNORECASE)
    parts = [normalize_ws(x) for x in re.split(r"[,&/]|(?:\s+\+\s+)|(?:\s{2,})", t) if normalize_ws(x)]
    return parts


def resolve_target_to_bus_ids(target: str, alias_to_bus: Dict[str, str], buses: Sequence[BusSpec]) -> List[str]:
    norm = normalize_text(target)
    if not norm:
        return []
    # exact alias first
    if norm in alias_to_bus:
        return [alias_to_bus[norm]]
    # fuzzy contains alias key
    matches: List[Tuple[int, str]] = []
    for alias, bus_id in alias_to_bus.items():
        if alias and alias in norm:
            matches.append((len(alias), bus_id))
    if matches:
        matches.sort(reverse=True)
        return [matches[0][1]]
    # direct bus id fallback
    for b in buses:
        if normalize_text(b.id) == norm:
            return [b.id]
    return []


def parse_bar_range(line: str) -> Optional[Tuple[int, int]]:
    patterns = [
        r"\b(?:bars?|measures?|mm?)\s*(\d+)\s*(?:-|to|through|thru)\s*(\d+)\b",
        r"\b(\d+)\s*(?:-|to|through|thru)\s*(\d+)\b",
    ]
    for pat in patterns:
        m = re.search(pat, line, flags=re.IGNORECASE)
        if m:
            a = int(m.group(1))
            b = int(m.group(2))
            return (min(a, b), max(a, b))
    return None


def parse_directive_line(
    line_no: int,
    line: str,
    alias_to_bus: Dict[str, str],
    buses: Sequence[BusSpec],
) -> Directive:
    raw = normalize_ws(line)
    br = parse_bar_range(raw)
    if br is None:
        raise ValueError(f"Line {line_no}: missing bar range ('bars 12-16').")
    start_bar, end_bar = br

    # Remove a leading "bars/measures x-y" phrase if present.
    body = re.sub(
        r"^\s*(?:bars?|measures?|mm?)\s*\d+\s*(?:-|to|through|thru)\s*\d+\s*[:,]?\s*",
        "",
        raw,
        flags=re.IGNORECASE,
    )

    explicit: Dict[str, str] = {}
    rest_role: Optional[str] = None

    rest_match = re.search(
        r"\b(?:the\s+rest|rest\s+of\s+(?:the\s+)?(?:orchestra|ensemble|score))\b.*?\b(primary|counterpoint|secondary|accompaniment|background|lead|support)\b",
        body,
        flags=re.IGNORECASE,
    )
    if rest_match:
        rest_role = normalize_role(rest_match.group(1))

    clause_re = re.compile(
        r"(?P<targets>.+?)\bto\s+be\s+(?P<role>primary|counterpoint|secondary|accompaniment|background|lead|support)\b",
        flags=re.IGNORECASE,
    )
    pos = 0
    while True:
        m = clause_re.search(body, pos)
        if not m:
            break
        targets_raw = normalize_ws(m.group("targets").strip(" ,;:"))
        role = normalize_role(m.group("role"))
        pos = m.end()
        if role is None:
            continue
        if re.search(r"\b(?:the\s+rest|rest\s+of)\b", targets_raw, flags=re.IGNORECASE):
            rest_role = role
            continue
        targets = split_targets(targets_raw)
        for t in targets:
            bus_ids = resolve_target_to_bus_ids(t, alias_to_bus, buses)
            for bid in bus_ids:
                explicit[bid] = role

    # Canonical compact syntax fallback:
    # bars 12-16: V1=PRIMARY, HN=SECONDARY, REST=ACCOMPANIMENT
    if not explicit and rest_role is None:
        kv_re = re.compile(
            r"([A-Za-z0-9_ ()/+-]+)\s*(?:=|->|as)\s*(primary|counterpoint|secondary|accompaniment|background|lead|support)",
            flags=re.IGNORECASE,
        )
        for m in kv_re.finditer(body):
            key = normalize_ws(m.group(1))
            role = normalize_role(m.group(2))
            if role is None:
                continue
            if normalize_text(key) in {"rest", "the rest", "rest of orchestra", "rest of the orchestra"}:
                rest_role = role
                continue
            for token in split_targets(key):
                for bid in resolve_target_to_bus_ids(token, alias_to_bus, buses):
                    explicit[bid] = role

    if not explicit and rest_role is None:
        raise ValueError(f"Line {line_no}: could not parse role assignments.")

    return Directive(
        line_no=line_no,
        raw=raw,
        start_bar=start_bar,
        end_bar=end_bar,
        explicit=explicit,
        rest_role=rest_role,
    )


def parse_directives(
    directives_path: Path,
    alias_to_bus: Dict[str, str],
    buses: Sequence[BusSpec],
) -> List[Directive]:
    lines = directives_path.read_text(encoding="utf-8").splitlines()
    out: List[Directive] = []
    for i, raw in enumerate(lines, start=1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        out.append(parse_directive_line(i, line, alias_to_bus, buses))
    if not out:
        raise ValueError(f"No directives parsed from {directives_path}")
    return out


def db_to_cc(db: float, min_db: float, max_db: float) -> int:
    if max_db <= min_db:
        return 0
    clamped = max(min_db, min(max_db, db))
    norm = (clamped - min_db) / (max_db - min_db)
    return max(0, min(127, int(round(norm * 127.0))))


def ms_to_ticks_local(ms: float, tick_ref: int, tpq: int, tempo_map: Sequence[Tuple[int, int]]) -> int:
    return mpr.ms_to_ticks(ms, tick_ref=tick_ref, tpq=tpq, tempo_map=tempo_map)


def clamp_bar_range(start_bar: int, end_bar: int, max_bar: int) -> Tuple[int, int]:
    sb = max(1, start_bar)
    eb = max(sb, end_bar)
    if sb > max_bar:
        return max_bar, max_bar
    return sb, min(eb, max_bar)


def apply_directives_to_bus_bar_db(
    directives: Sequence[Directive],
    buses: Sequence[BusSpec],
    role_db_offsets: Dict[str, float],
    n_bars: int,
) -> Dict[str, List[float]]:
    bus_ids = [b.id for b in buses]
    db_by_bus: Dict[str, List[float]] = {b.id: [float(b.default_db) for _ in range(n_bars)] for b in buses}

    for d in directives:
        sb, eb = clamp_bar_range(d.start_bar, d.end_bar, n_bars)
        explicit_buses = set(d.explicit.keys())
        for bar in range(sb, eb + 1):
            bi = bar - 1
            for bid, role in d.explicit.items():
                if bid not in db_by_bus:
                    continue
                db_by_bus[bid][bi] = float(next((b.default_db for b in buses if b.id == bid), 0.0)) + float(
                    role_db_offsets[role]
                )
            if d.rest_role is not None:
                for bid in bus_ids:
                    if bid in explicit_buses:
                        continue
                    db_by_bus[bid][bi] = float(next((b.default_db for b in buses if b.id == bid), 0.0)) + float(
                        role_db_offsets[d.rest_role]
                    )
    return db_by_bus


def build_bus_cc_events(
    bus: BusSpec,
    bar_starts: Sequence[int],
    bus_bar_db: Sequence[float],
    defaults: GlobalDefaults,
    tpq: int,
    tempo_map: Sequence[Tuple[int, int]],
) -> List[Tuple[int, int]]:
    if not bus_bar_db:
        return []
    # Segment by equal db per bar.
    segments: List[Tuple[int, int, float]] = []
    seg_start_bar = 1
    cur_db = float(bus_bar_db[0])
    for i in range(2, len(bus_bar_db) + 1):
        db = float(bus_bar_db[i - 1])
        if abs(db - cur_db) < 1e-9:
            continue
        segments.append((seg_start_bar, i - 1, cur_db))
        seg_start_bar = i
        cur_db = db
    segments.append((seg_start_bar, len(bus_bar_db), cur_db))

    events: List[Tuple[int, int]] = []
    # Initial value.
    initial_cc = db_to_cc(segments[0][2], bus.min_db, bus.max_db)
    events.append((0, initial_cc))
    prev_db = segments[0][2]

    for seg_idx in range(1, len(segments)):
        start_bar, _, next_db = segments[seg_idx]
        boundary_tick = int(bar_starts[start_bar - 1])
        if abs(next_db - prev_db) < 1e-9:
            continue
        ramp_ticks = ms_to_ticks_local(defaults.ramp_in_ms, boundary_tick, tpq=tpq, tempo_map=tempo_map)
        step_ticks = max(1, ms_to_ticks_local(defaults.control_step_ms, boundary_tick, tpq=tpq, tempo_map=tempo_map))
        if ramp_ticks <= 0:
            events.append((boundary_tick, db_to_cc(next_db, bus.min_db, bus.max_db)))
            prev_db = next_db
            continue
        # Keep current at boundary, then ramp into next target.
        start_cc = db_to_cc(prev_db, bus.min_db, bus.max_db)
        end_cc = db_to_cc(next_db, bus.min_db, bus.max_db)
        events.append((boundary_tick, start_cc))
        n_steps = max(2, int(round(ramp_ticks / step_ticks)))
        for s in range(1, n_steps + 1):
            alpha = s / float(n_steps)
            tick = boundary_tick + int(round(alpha * ramp_ticks))
            cc = int(round(start_cc + (end_cc - start_cc) * alpha))
            events.append((tick, max(0, min(127, cc))))
        prev_db = next_db

    # Deduplicate same-tick and same-value runs.
    events.sort(key=lambda x: x[0])
    compact: List[Tuple[int, int]] = []
    for tick, cc in events:
        if compact and tick == compact[-1][0]:
            compact[-1] = (tick, cc)
            continue
        if compact and cc == compact[-1][1]:
            continue
        compact.append((tick, cc))
    return compact


def build_midi_track_with_cc(track_name: str, cc_num: int, channel_1based: int, events: Sequence[Tuple[int, int]]) -> mido.MidiTrack:
    tr = mido.MidiTrack()
    tr.append(mido.MetaMessage("track_name", name=track_name, time=0))
    last_tick = 0
    ch = max(0, min(15, int(channel_1based) - 1))
    for tick, value in sorted(events, key=lambda x: x[0]):
        delta = max(0, int(tick) - last_tick)
        tr.append(
            mido.Message(
                "control_change",
                channel=ch,
                control=int(cc_num),
                value=max(0, min(127, int(value))),
                time=delta,
            )
        )
        last_tick = int(tick)
    tr.append(mido.MetaMessage("end_of_track", time=0))
    return tr


def build_meta_track(meta_events: Sequence[Tuple[int, int, mido.MetaMessage]], end_tick: int) -> mido.MidiTrack:
    tr = mido.MidiTrack()
    tr.append(mido.MetaMessage("track_name", name="MixCT Conductor", time=0))
    last_tick = 0
    for tick, _, msg in sorted(meta_events, key=lambda x: (x[0], x[1])):
        delta = max(0, int(tick) - last_tick)
        tr.append(msg.copy(time=delta))
        last_tick = int(tick)
    tr.append(mido.MetaMessage("end_of_track", time=max(0, int(end_tick) - last_tick)))
    return tr


def write_plan_csv(
    path: Path,
    buses: Sequence[BusSpec],
    bar_starts: Sequence[int],
    db_by_bus: Dict[str, Sequence[float]],
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    n_bars = len(bar_starts) - 1
    with path.open("w", newline="", encoding="utf-8") as f:
        w = csv.writer(f)
        header = ["bar", "start_tick", "end_tick"] + [b.id for b in buses]
        w.writerow(header)
        for bar in range(1, n_bars + 1):
            row = [bar, int(bar_starts[bar - 1]), int(bar_starts[bar])]
            for b in buses:
                row.append(f"{float(db_by_bus[b.id][bar - 1]):.2f}")
            w.writerow(row)


def parse_args() -> argparse.Namespace:
    ap = argparse.ArgumentParser(description="MixCT MVP-A offline bus automation generator for DP.")
    ap.add_argument("--source-midi", required=True, type=Path, help="DP-ready source MIDI with conductor metadata.")
    ap.add_argument("--session-map", required=True, type=Path, help="MixCT session-map YAML.")
    ap.add_argument("--directives", required=True, type=Path, help="Text directives file.")
    ap.add_argument("--output-dir", required=True, type=Path, help="Output directory.")
    return ap.parse_args()


def main() -> int:
    args = parse_args()
    source_midi = args.source_midi if args.source_midi.is_absolute() else (Path.cwd() / args.source_midi)
    session_map_path = args.session_map if args.session_map.is_absolute() else (Path.cwd() / args.session_map)
    directives_path = args.directives if args.directives.is_absolute() else (Path.cwd() / args.directives)
    out_dir = args.output_dir if args.output_dir.is_absolute() else (Path.cwd() / args.output_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    if not source_midi.exists():
        raise FileNotFoundError(f"Missing source MIDI: {source_midi}")
    if not session_map_path.exists():
        raise FileNotFoundError(f"Missing session map: {session_map_path}")
    if not directives_path.exists():
        raise FileNotFoundError(f"Missing directives file: {directives_path}")

    buses, alias_to_bus, role_offsets, defaults = parse_session_map(session_map_path)
    directives = parse_directives(directives_path, alias_to_bus=alias_to_bus, buses=buses)

    mid = mido.MidiFile(source_midi)
    tempo_map = mpr.build_tempo_map(mid)
    bar_starts = build_bar_starts(mid)
    n_bars = len(bar_starts) - 1

    track_names = list_main_instrument_tracks(mid)
    track_map = assign_tracks_to_buses(track_names, buses=buses)

    db_by_bus = apply_directives_to_bus_bar_db(
        directives=directives,
        buses=buses,
        role_db_offsets=role_offsets,
        n_bars=n_bars,
    )

    bus_cc_events: Dict[str, List[Tuple[int, int]]] = {}
    max_tick = 0
    for b in buses:
        ev = build_bus_cc_events(
            bus=b,
            bar_starts=bar_starts,
            bus_bar_db=db_by_bus[b.id],
            defaults=defaults,
            tpq=int(mid.ticks_per_beat),
            tempo_map=tempo_map,
        )
        bus_cc_events[b.id] = ev
        if ev:
            max_tick = max(max_tick, ev[-1][0])

    out_mid = mido.MidiFile(type=1, ticks_per_beat=mid.ticks_per_beat)
    meta_events = collect_global_meta_events(mid)
    out_mid.tracks.append(build_meta_track(meta_events, end_tick=max_tick + int(mid.ticks_per_beat * 4)))
    for b in buses:
        tname = f"MixCT {b.id} ({b.label})"
        out_mid.tracks.append(
            build_midi_track_with_cc(
                track_name=tname,
                cc_num=b.cc,
                channel_1based=b.channel_1based,
                events=bus_cc_events[b.id],
            )
        )

    midi_out = out_dir / f"{source_midi.stem}__MIXCT_AUTOMATION.mid"
    plan_csv = out_dir / f"{source_midi.stem}__MIXCT_PLAN.csv"
    audit_json = out_dir / f"{source_midi.stem}__MIXCT_AUDIT.json"

    out_mid.save(midi_out)
    write_plan_csv(plan_csv, buses=buses, bar_starts=bar_starts, db_by_bus=db_by_bus)

    audit = {
        "tool": "mixct-mvp-a-cli",
        "source_midi": str(source_midi),
        "session_map": str(session_map_path),
        "directives": str(directives_path),
        "output_midi": str(midi_out),
        "output_plan_csv": str(plan_csv),
        "ticks_per_beat": int(mid.ticks_per_beat),
        "bars_covered": int(n_bars),
        "buses": [
            {
                "id": b.id,
                "label": b.label,
                "cc": b.cc,
                "channel": b.channel_1based,
                "default_db": b.default_db,
                "assigned_tracks": track_map.get(b.id, []),
                "event_count": len(bus_cc_events[b.id]),
            }
            for b in buses
        ],
        "directives": [
            {
                "line_no": d.line_no,
                "raw": d.raw,
                "start_bar": d.start_bar,
                "end_bar": d.end_bar,
                "explicit": d.explicit,
                "rest_role": d.rest_role,
            }
            for d in directives
        ],
        "role_db_offsets": role_offsets,
        "defaults": {
            "ramp_in_ms": defaults.ramp_in_ms,
            "control_step_ms": defaults.control_step_ms,
        },
    }
    audit_json.write_text(json.dumps(audit, indent=2), encoding="utf-8")

    print(f"Wrote: {midi_out}")
    print(f"Wrote: {plan_csv}")
    print(f"Wrote: {audit_json}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
