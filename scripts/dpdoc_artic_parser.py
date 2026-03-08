#!/usr/bin/env python3
"""
Heuristic parser for DP .dpdoc articulation payloads.

Current scope:
- Parse "Pencil Articulations" blocks.
- Infer per-note articulation records using embedded IDs + tick markers.
- Emit machine-readable audit files for validation.

Notes:
- DP .dpdoc is a binary format and not fully documented.
- This parser is intentionally conservative and reports confidence per record.
"""

from __future__ import annotations

import argparse
import csv
import json
import re
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Dict, Iterable, List, Optional, Tuple


PENCIL_MARKER = b"Pencil Articulations"
GUID_CODE_RE = re.compile(rb"\x02(?P<guid>.{16})\xe5\xcc\xc8(?P<code>.)", re.DOTALL)
PRIMARY_REC_RE = re.compile(rb"\x02(?P<guid>.{16})\xe5\xcc(?P<series>.)(?P<code>.)", re.DOTALL)
TICK_MARKER_RE = re.compile(rb"\x00\x00\x00(?P<kind>[\x12\x13])\x00\x00(?P<raw>.{4})", re.DOTALL)
ASCII_RE = re.compile(rb"[ -~]{8,}")


@dataclass
class ParsedEvent:
    source_offset: int
    note_index_inferred: int
    start_tick_inferred: Optional[int]
    marker_kind: Optional[int]
    articulation_code: Optional[int]
    articulation_record_guid_hex: Optional[str]
    articulation_name_hint: Optional[str]
    articulation_name_resolved: Optional[str]
    mapping_source: Optional[str]
    series_id: Optional[int]
    primary_record_offset: Optional[int]
    active_layer: bool
    confidence: str


def _find_all(haystack: bytes, needle: bytes) -> List[int]:
    out: List[int] = []
    start = 0
    while True:
        idx = haystack.find(needle, start)
        if idx < 0:
            return out
        out.append(idx)
        start = idx + 1


def _clean_name_hint(s: str) -> str:
    # DP often prefixes label runs with one non-letter char in this payload.
    while s and not s[0].isalpha():
        s = s[1:]
    return s.strip()


def _extract_name_hint(block: bytes) -> Optional[str]:
    strings = [m.group().decode("latin1", errors="ignore") for m in ASCII_RE.finditer(block)]
    for s in strings:
        if s == "Pencil Articulations":
            continue
        cleaned = _clean_name_hint(s)
        if " - " in cleaned and len(cleaned) >= 12:
            return cleaned
    return None


def _decode_tick(raw4: bytes) -> int:
    # Observed pattern stores tick value in the high 16 bits of this 32-bit field.
    return int.from_bytes(raw4, "big") >> 16


def _first_valid_tick(block: bytes) -> tuple[Optional[int], Optional[int]]:
    for m in TICK_MARKER_RE.finditer(block):
        kind = m.group("kind")[0]
        tick = _decode_tick(m.group("raw"))
        # Defensive range gate for musical ticks; ignores obvious structural noise.
        if 0 <= tick <= 2_000_000:
            return tick, kind
    return None, None


def _confidence(
    guid_hex: Optional[str],
    code: Optional[int],
    tick: Optional[int],
    name_hint: Optional[str],
) -> str:
    score = 0
    if guid_hex:
        score += 1
    if code is not None:
        score += 1
    if tick is not None:
        score += 1
    if name_hint:
        score += 1
    if score >= 4:
        return "high"
    if score >= 3:
        return "medium"
    return "low"


def parse_dpdoc(data: bytes) -> List[ParsedEvent]:
    marker_offsets = _find_all(data, PENCIL_MARKER)
    if not marker_offsets:
        return []

    events: List[ParsedEvent] = []
    for i, start in enumerate(marker_offsets):
        end = marker_offsets[i + 1] if i + 1 < len(marker_offsets) else min(len(data), start + 12_000)
        block = data[start:end]

        guid_hex: Optional[str] = None
        code: Optional[int] = None
        series_id: Optional[int] = None
        primary_record_offset: Optional[int] = None
        # In fixtures, the primary articulation record appears near start of the block.
        m = PRIMARY_REC_RE.search(block[:512])
        if m:
            guid_hex = m.group("guid").hex()
            series_id = m.group("series")[0]
            code = m.group("code")[0]
            primary_record_offset = m.start()
        else:
            # Fallback for older pattern variants.
            m2 = GUID_CODE_RE.search(block)
            if m2:
                guid_hex = m2.group("guid").hex()
                code = m2.group("code")[0]

        tick, marker_kind = _first_valid_tick(block)
        name_hint = _extract_name_hint(block)
        conf = _confidence(guid_hex, code, tick, name_hint)

        events.append(
            ParsedEvent(
                source_offset=start,
                note_index_inferred=-1,
                start_tick_inferred=tick,
                marker_kind=marker_kind,
                articulation_code=code,
                articulation_record_guid_hex=guid_hex,
                articulation_name_hint=name_hint,
                articulation_name_resolved=None,
                mapping_source=None,
                series_id=series_id,
                primary_record_offset=primary_record_offset,
                active_layer=False,
                confidence=conf,
            )
        )

    # Establish musical order by inferred tick when available, else by file order.
    sortable = sorted(
        enumerate(events),
        key=lambda x: (
            x[1].start_tick_inferred if x[1].start_tick_inferred is not None else 10**12,
            x[1].source_offset,
        ),
    )
    for order_idx, (orig_idx, _) in enumerate(sortable):
        events[orig_idx].note_index_inferred = order_idx

    return sorted(events, key=lambda e: e.note_index_inferred)


def choose_active_series(events: List[ParsedEvent]) -> Optional[int]:
    stats: Dict[int, dict] = {}
    for e in events:
        if e.series_id is None:
            continue
        s = stats.setdefault(e.series_id, {"count": 0, "max_offset": -1})
        s["count"] += 1
        if e.source_offset > s["max_offset"]:
            s["max_offset"] = e.source_offset
    if not stats:
        return None
    # Prefer the newest serialized layer; tie-break by count then series id.
    ordered = sorted(
        stats.items(),
        key=lambda kv: (kv[1]["max_offset"], kv[1]["count"], kv[0]),
        reverse=True,
    )
    return ordered[0][0]


def apply_active_layer(events: List[ParsedEvent]) -> Optional[int]:
    chosen = choose_active_series(events)
    if chosen is None:
        return None
    for e in events:
        e.active_layer = e.series_id == chosen
    return chosen


def backfill_active_ticks(events: List[ParsedEvent]) -> None:
    active = [e for e in events if e.active_layer]
    if not active:
        return

    ref = [e for e in events if (not e.active_layer) and e.start_tick_inferred is not None]
    if not ref:
        return

    ref_sorted = sorted(ref, key=lambda e: (e.start_tick_inferred or 10**12, e.source_offset))
    act_sorted = sorted(active, key=lambda e: e.source_offset)
    if len(ref_sorted) < len(act_sorted):
        # Only partial backfill possible.
        for i, e in enumerate(act_sorted):
            if e.start_tick_inferred is None and i < len(ref_sorted):
                e.start_tick_inferred = ref_sorted[i].start_tick_inferred
        return

    known = [e for e in act_sorted if e.start_tick_inferred is not None]
    # If active layer exposes sparse or noisy timing, align positionally to reference layer.
    # This preserves ordering while removing serializer jitter (e.g., tick=5 instead of 0).
    if len(known) <= (len(act_sorted) // 2):
        for i, e in enumerate(act_sorted):
            e.start_tick_inferred = ref_sorted[i].start_tick_inferred
        return

    # Otherwise only fill missing values.
    for i, e in enumerate(act_sorted):
        if e.start_tick_inferred is None:
            e.start_tick_inferred = ref_sorted[i].start_tick_inferred


def load_dpartmap_names(dpartmap_path: Path) -> List[str]:
    obj = json.loads(dpartmap_path.read_text(encoding="utf-8"))
    arts = obj.get("articulations", [])
    names: List[str] = []
    for art in arts:
        name = art.get("name")
        if isinstance(name, str) and name.strip():
            names.append(name.strip())
    return names


def load_calibration(
    calibration_path: Optional[Path],
) -> tuple[Dict[int, str], Dict[str, str], Dict[Tuple[int, int], str]]:
    if calibration_path is None:
        return {}, {}, {}
    obj = json.loads(calibration_path.read_text(encoding="utf-8"))
    code_map_raw = obj.get("code_to_name", {})
    guid_map_raw = obj.get("guid_to_name", {})
    series_code_map_raw = obj.get("series_code_to_name", {})
    code_map: Dict[int, str] = {}
    guid_map: Dict[str, str] = {}
    series_code_map: Dict[Tuple[int, int], str] = {}
    if isinstance(code_map_raw, dict):
        for k, v in code_map_raw.items():
            try:
                code_map[int(k)] = str(v)
            except Exception:
                continue
    if isinstance(guid_map_raw, dict):
        for k, v in guid_map_raw.items():
            if isinstance(k, str) and isinstance(v, str):
                guid_map[k.lower()] = v
    if isinstance(series_code_map_raw, dict):
        for skey, cmap in series_code_map_raw.items():
            try:
                s = int(skey)
            except Exception:
                continue
            if not isinstance(cmap, dict):
                continue
            for ckey, v in cmap.items():
                try:
                    c = int(ckey)
                except Exception:
                    continue
                if isinstance(v, str) and v.strip():
                    series_code_map[(s, c)] = v.strip()
    return code_map, guid_map, series_code_map


def build_assumed_code_map(
    events: List[ParsedEvent],
    dpartmap_names: List[str],
    start_index: int,
    max_items: Optional[int],
    order_mode: str,
) -> Dict[int, str]:
    codes = sorted({e.articulation_code for e in events if e.articulation_code is not None})
    if order_mode == "tick":
        first_at: Dict[int, int] = {}
        for e in events:
            if e.articulation_code is None:
                continue
            tick = e.start_tick_inferred if e.start_tick_inferred is not None else 10**12
            old = first_at.get(e.articulation_code)
            if old is None or tick < old:
                first_at[e.articulation_code] = tick
        codes = sorted(codes, key=lambda c: first_at.get(c, 10**12))
    if max_items is not None:
        codes = codes[: max(0, max_items)]
    out: Dict[int, str] = {}
    for i, code in enumerate(codes):
        idx = start_index + i
        if 0 <= idx < len(dpartmap_names):
            out[code] = dpartmap_names[idx]
    return out


def apply_calibration(
    events: List[ParsedEvent],
    code_map: Dict[int, str],
    guid_map: Dict[str, str],
    series_code_map: Dict[Tuple[int, int], str],
) -> None:
    for e in events:
        if e.articulation_record_guid_hex:
            g = e.articulation_record_guid_hex.lower()
            if g in guid_map:
                e.articulation_name_resolved = guid_map[g]
                e.mapping_source = "guid_map"
                continue
        if e.series_id is not None and e.articulation_code is not None:
            key = (e.series_id, e.articulation_code)
            if key in series_code_map:
                e.articulation_name_resolved = series_code_map[key]
                e.mapping_source = "series_code_map"
                continue
        if e.articulation_code is not None and e.articulation_code in code_map:
            e.articulation_name_resolved = code_map[e.articulation_code]
            e.mapping_source = "code_map"
            continue
        if e.articulation_name_hint:
            e.articulation_name_resolved = e.articulation_name_hint
            e.mapping_source = "hint"
            continue
        e.articulation_name_resolved = None
        e.mapping_source = None


def _write_csv(path: Path, rows: Iterable[ParsedEvent]) -> None:
    fieldnames = [
        "note_index_inferred",
        "start_tick_inferred",
        "source_offset",
        "series_id",
        "active_layer",
        "marker_kind",
        "articulation_code",
        "articulation_record_guid_hex",
        "primary_record_offset",
        "articulation_name_hint",
        "articulation_name_resolved",
        "mapping_source",
        "confidence",
    ]
    with path.open("w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=fieldnames)
        w.writeheader()
        for row in rows:
            w.writerow(
                {
                    "note_index_inferred": row.note_index_inferred,
                    "start_tick_inferred": row.start_tick_inferred,
                    "source_offset": row.source_offset,
                    "series_id": row.series_id,
                    "active_layer": int(row.active_layer),
                    "marker_kind": row.marker_kind,
                    "articulation_code": row.articulation_code,
                    "articulation_record_guid_hex": row.articulation_record_guid_hex,
                    "primary_record_offset": row.primary_record_offset,
                    "articulation_name_hint": row.articulation_name_hint or "",
                    "articulation_name_resolved": row.articulation_name_resolved or "",
                    "mapping_source": row.mapping_source or "",
                    "confidence": row.confidence,
                }
            )


def parse_args() -> argparse.Namespace:
    ap = argparse.ArgumentParser(description="Parse inferred articulation note tags from a DP .dpdoc file")
    ap.add_argument("--dpdoc", required=True, help="Path to target .dpdoc file")
    ap.add_argument("--output-dir", required=True, help="Output directory")
    ap.add_argument("--dpartmap", help="Optional .dpartmap JSON path for articulation names")
    ap.add_argument("--calibration", help="Optional calibration JSON with code_to_name / guid_to_name")
    ap.add_argument(
        "--assume-map-order",
        action="store_true",
        help=(
            "Assume unresolved articulation codes map to .dpartmap names in order "
            "(for controlled calibration fixtures)."
        ),
    )
    ap.add_argument(
        "--assume-start-index",
        type=int,
        default=0,
        help="Start index in .dpartmap articulation list when --assume-map-order is used",
    )
    ap.add_argument(
        "--assume-max-items",
        type=int,
        default=0,
        help=(
            "Max number of assumed code mappings. 0 means map all detected codes when "
            "--assume-map-order is used."
        ),
    )
    ap.add_argument(
        "--assume-code-order",
        choices=["code", "tick"],
        default="code",
        help="Ordering for detected codes under --assume-map-order",
    )
    return ap.parse_args()


def main() -> None:
    args = parse_args()
    dpdoc = Path(args.dpdoc).expanduser().resolve()
    out_dir = Path(args.output_dir).expanduser().resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    data = dpdoc.read_bytes()
    events = parse_dpdoc(data)
    chosen_series = apply_active_layer(events)
    backfill_active_ticks(events)

    dpartmap_names: List[str] = []
    if args.dpartmap:
        dpartmap_names = load_dpartmap_names(Path(args.dpartmap).expanduser().resolve())

    code_map, guid_map, series_code_map = load_calibration(
        Path(args.calibration).expanduser().resolve() if args.calibration else None
    )
    assumed_code_map: Dict[int, str] = {}
    if args.assume_map_order and dpartmap_names:
        max_items = args.assume_max_items if args.assume_max_items > 0 else None
        assumed_code_map = build_assumed_code_map(
            events=events,
            dpartmap_names=dpartmap_names,
            start_index=max(0, args.assume_start_index),
            max_items=max_items,
            order_mode=args.assume_code_order,
        )
        for code, name in assumed_code_map.items():
            code_map.setdefault(code, name)

    apply_calibration(
        events,
        code_map=code_map,
        guid_map=guid_map,
        series_code_map=series_code_map,
    )

    active_events = [e for e in events if e.active_layer]
    active_events_sorted = sorted(
        active_events,
        key=lambda e: (
            e.start_tick_inferred if e.start_tick_inferred is not None else 10**12,
            e.source_offset,
        ),
    )
    for idx, e in enumerate(active_events_sorted):
        e.note_index_inferred = idx

    summary = {
        "input_dpdoc": str(dpdoc),
        "input_size_bytes": len(data),
        "pencil_block_count": len(events),
        "events_with_tick": sum(1 for e in events if e.start_tick_inferred is not None),
        "events_with_code": sum(1 for e in events if e.articulation_code is not None),
        "events_with_name_hint": sum(1 for e in events if e.articulation_name_hint),
        "events_with_resolved_name": sum(1 for e in events if e.articulation_name_resolved),
        "series_counts": {
            str(s): sum(1 for e in events if e.series_id == s)
            for s in sorted({e.series_id for e in events if e.series_id is not None})
        },
        "active_series_id": chosen_series,
        "active_event_count": len(active_events),
        "active_events_with_tick": sum(1 for e in active_events if e.start_tick_inferred is not None),
        "active_events_with_resolved_name": sum(1 for e in active_events if e.articulation_name_resolved),
        "mapping_source_counts": {
            "hint": sum(1 for e in events if e.mapping_source == "hint"),
            "guid_map": sum(1 for e in events if e.mapping_source == "guid_map"),
            "series_code_map": sum(1 for e in events if e.mapping_source == "series_code_map"),
            "code_map": sum(1 for e in events if e.mapping_source == "code_map"),
            "unresolved": sum(1 for e in events if not e.mapping_source),
        },
        "assumed_code_map": {str(k): v for k, v in sorted(assumed_code_map.items())},
        "confidence_counts": {
            "high": sum(1 for e in events if e.confidence == "high"),
            "medium": sum(1 for e in events if e.confidence == "medium"),
            "low": sum(1 for e in events if e.confidence == "low"),
        },
    }

    (out_dir / "parsed_articulation_events.json").write_text(
        json.dumps([asdict(e) for e in events], indent=2), encoding="utf-8"
    )
    _write_csv(out_dir / "parsed_articulation_events.csv", events)
    (out_dir / "parsed_articulation_events_active.json").write_text(
        json.dumps([asdict(e) for e in active_events_sorted], indent=2), encoding="utf-8"
    )
    _write_csv(out_dir / "parsed_articulation_events_active.csv", active_events_sorted)
    (out_dir / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    # Emit a calibration template to simplify manual refinement.
    unique_codes = sorted({e.articulation_code for e in events if e.articulation_code is not None})
    unique_series = sorted({e.series_id for e in events if e.series_id is not None})
    unique_guids = sorted(
        {e.articulation_record_guid_hex for e in events if e.articulation_record_guid_hex is not None}
    )
    calib_template = {
        "code_to_name": {str(c): "" for c in unique_codes},
        "series_code_to_name": {
            str(s): {
                str(c): ""
                for c in sorted(
                    {e.articulation_code for e in events if e.series_id == s and e.articulation_code is not None}
                )
            }
            for s in unique_series
        },
        "guid_to_name": {g: "" for g in unique_guids},
    }
    (out_dir / "calibration_template.json").write_text(json.dumps(calib_template, indent=2), encoding="utf-8")

    print("Wrote:")
    print(f"  {out_dir / 'summary.json'}")
    print(f"  {out_dir / 'parsed_articulation_events.csv'}")
    print(f"  {out_dir / 'parsed_articulation_events.json'}")
    print(f"  {out_dir / 'parsed_articulation_events_active.csv'}")
    print(f"  {out_dir / 'parsed_articulation_events_active.json'}")
    print(f"  {out_dir / 'calibration_template.json'}")
    print("")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
