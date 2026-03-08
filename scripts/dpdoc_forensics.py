#!/usr/bin/env python3
"""
Forensic differ for Digital Performer .dpdoc binaries.

Goal:
- isolate likely "semantic" edit regions (e.g., one articulation change)
- while discounting "save noise" introduced by DP during normal save

Method:
1) Compare base -> saveonly to learn save noise.
2) Compare saveonly -> oneartic to find target edits.
3) Score target edits by overlap/proximity with save-noise regions.
4) Emit ranked candidates + side-by-side context for manual analysis.

Expected inputs:
- base.dpdoc: reference project
- saveonly.dpdoc: base opened/saved with no musical changes
- oneartic.dpdoc: base with exactly one articulation changed, then saved
"""

from __future__ import annotations

import argparse
import csv
import json
from dataclasses import dataclass
from difflib import SequenceMatcher
from pathlib import Path
from typing import Dict, List, Sequence, Tuple


@dataclass
class DiffRun:
    tag: str
    a0: int
    a1: int
    b0: int
    b1: int

    @property
    def len_a(self) -> int:
        return max(0, self.a1 - self.a0)

    @property
    def len_b(self) -> int:
        return max(0, self.b1 - self.b0)

    def as_dict(self) -> Dict[str, int | str]:
        return {
            "tag": self.tag,
            "a0": self.a0,
            "a1": self.a1,
            "b0": self.b0,
            "b1": self.b1,
            "len_a": self.len_a,
            "len_b": self.len_b,
        }


def read_bytes(path: Path) -> bytes:
    return path.read_bytes()


def _chunkify(data: bytes, block_size: int) -> List[bytes]:
    return [data[i : i + block_size] for i in range(0, len(data), block_size)]


def diff_runs(a: bytes, b: bytes, block_size: int) -> List[DiffRun]:
    """
    Compute non-equal opcode runs using block-level matching for speed.
    Returned offsets are byte offsets in original buffers.
    """
    if block_size < 1:
        block_size = 1
    if block_size == 1:
        sm = SequenceMatcher(None, a, b, autojunk=False)
        out: List[DiffRun] = []
        for tag, a0, a1, b0, b1 in sm.get_opcodes():
            if tag == "equal":
                continue
            out.append(DiffRun(tag=tag, a0=a0, a1=a1, b0=b0, b1=b1))
        return out

    a_chunks = _chunkify(a, block_size)
    b_chunks = _chunkify(b, block_size)
    sm = SequenceMatcher(None, a_chunks, b_chunks, autojunk=False)
    out: List[DiffRun] = []
    for tag, ca0, ca1, cb0, cb1 in sm.get_opcodes():
        if tag == "equal":
            continue
        a0 = min(len(a), ca0 * block_size)
        a1 = min(len(a), ca1 * block_size)
        b0 = min(len(b), cb0 * block_size)
        b1 = min(len(b), cb1 * block_size)
        out.append(DiffRun(tag=tag, a0=a0, a1=a1, b0=b0, b1=b1))
    return out


def total_changed_bytes(runs: Sequence[DiffRun], side: str = "a") -> int:
    if side == "a":
        return sum(r.len_a for r in runs)
    return sum(r.len_b for r in runs)


def interval_overlap(a0: int, a1: int, b0: int, b1: int) -> int:
    return max(0, min(a1, b1) - max(a0, b0))


def printable_ascii(buf: bytes) -> str:
    chars: List[str] = []
    for bt in buf:
        if 32 <= bt <= 126:
            chars.append(chr(bt))
        else:
            chars.append(".")
    return "".join(chars)


def nearby_ascii(data: bytes, start: int, end: int, window: int = 96) -> str:
    lo = max(0, start - window)
    hi = min(len(data), end + window)
    return printable_ascii(data[lo:hi])


def nearby_hex(data: bytes, start: int, end: int, window: int = 32) -> str:
    lo = max(0, start - window)
    hi = min(len(data), end + window)
    return data[lo:hi].hex()


def score_target_run_against_noise(
    target: DiffRun,
    noise_runs_in_save_coords: Sequence[DiffRun],
    insertion_tolerance: int = 32,
) -> Tuple[float, bool]:
    """
    Returns:
    - overlap_ratio: how much of target's save-side span overlaps save-noise spans
    - near_insertion_noise: True if insertion target is near insertion save-noise
    """
    t0, t1 = target.a0, target.a1  # save->oneartic: a-side is saveonly coords
    tlen = max(1, t1 - t0)

    overlap = 0
    near_insert_noise = False

    for n in noise_runs_in_save_coords:
        n0, n1 = n.a0, n.a1  # save->base: a-side is saveonly coords
        if t0 == t1 and n0 == n1:
            if abs(t0 - n0) <= insertion_tolerance:
                near_insert_noise = True
            continue
        overlap += interval_overlap(t0, t1, n0, n1)

    overlap_ratio = min(1.0, overlap / float(tlen))
    return overlap_ratio, near_insert_noise


def write_csv(path: Path, rows: List[Dict[str, object]], fieldnames: List[str]) -> None:
    with path.open("w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=fieldnames)
        w.writeheader()
        for r in rows:
            w.writerow(r)


def parse_args() -> argparse.Namespace:
    ap = argparse.ArgumentParser(description="Forensic diff for DP .dpdoc articulation isolation")
    ap.add_argument("--base", required=True, help="Base .dpdoc path")
    ap.add_argument("--saveonly", required=True, help="Save-only .dpdoc path (no musical change)")
    ap.add_argument("--oneartic", required=True, help="One-articulation-change .dpdoc path")
    ap.add_argument("--output-dir", required=True, help="Output directory for report artifacts")
    ap.add_argument("--top", type=int, default=80, help="Top candidate runs to emit in ranked CSV")
    ap.add_argument(
        "--noise-overlap-threshold",
        type=float,
        default=0.50,
        help="Max overlap ratio with save-noise to retain candidate",
    )
    ap.add_argument(
        "--insertion-tolerance",
        type=int,
        default=32,
        help="Byte tolerance for insertion-vs-insertion noise matching",
    )
    ap.add_argument(
        "--block-size",
        type=int,
        default=16,
        help="Diff token size in bytes (1 is exact but slower; 16 is usually fast/stable)",
    )
    return ap.parse_args()


def main() -> None:
    args = parse_args()
    base_path = Path(args.base).expanduser().resolve()
    save_path = Path(args.saveonly).expanduser().resolve()
    one_path = Path(args.oneartic).expanduser().resolve()
    out_dir = Path(args.output_dir).expanduser().resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    base = read_bytes(base_path)
    save = read_bytes(save_path)
    one = read_bytes(one_path)

    # Baseline save-noise:
    # save -> base so opcodes' a-side coordinates are in save-space
    block_size = max(1, int(args.block_size))
    noise_runs = diff_runs(save, base, block_size=block_size)
    # Target signal:
    # save -> one so opcodes' a-side coordinates are in save-space
    target_runs = diff_runs(save, one, block_size=block_size)
    # Reference totals:
    base_to_one_runs = diff_runs(base, one, block_size=block_size)

    all_rows: List[Dict[str, object]] = []
    candidates: List[Dict[str, object]] = []

    for idx, tr in enumerate(target_runs):
        overlap_ratio, near_insert_noise = score_target_run_against_noise(
            target=tr,
            noise_runs_in_save_coords=noise_runs,
            insertion_tolerance=max(0, args.insertion_tolerance),
        )

        a_ascii = nearby_ascii(save, tr.a0, tr.a1)
        b_ascii = nearby_ascii(one, tr.b0, tr.b1)

        row: Dict[str, object] = {
            "idx": idx,
            "tag": tr.tag,
            "save_a0": tr.a0,
            "save_a1": tr.a1,
            "save_len": tr.len_a,
            "one_b0": tr.b0,
            "one_b1": tr.b1,
            "one_len": tr.len_b,
            "overlap_ratio_with_save_noise": round(overlap_ratio, 6),
            "near_insertion_noise": int(near_insert_noise),
            "size_score": max(tr.len_a, tr.len_b),
            "save_ascii_context": a_ascii,
            "one_ascii_context": b_ascii,
            "save_hex_context": nearby_hex(save, tr.a0, tr.a1),
            "one_hex_context": nearby_hex(one, tr.b0, tr.b1),
        }
        all_rows.append(row)

        if overlap_ratio <= float(args.noise_overlap_threshold) and not near_insert_noise:
            candidates.append(row)

    candidates_sorted = sorted(
        candidates,
        key=lambda r: (
            float(r["overlap_ratio_with_save_noise"]),
            -int(r["size_score"]),
            int(r["save_a0"]),
        ),
    )

    if args.top > 0:
        candidates_sorted = candidates_sorted[: args.top]

    summary = {
        "inputs": {
            "base": str(base_path),
            "saveonly": str(save_path),
            "oneartic": str(one_path),
        },
        "sizes": {
            "base_bytes": len(base),
            "saveonly_bytes": len(save),
            "oneartic_bytes": len(one),
        },
        "diff_totals": {
            "save_to_base_runs": len(noise_runs),
            "save_to_base_changed_bytes_in_save_space": total_changed_bytes(noise_runs, side="a"),
            "save_to_one_runs": len(target_runs),
            "save_to_one_changed_bytes_in_save_space": total_changed_bytes(target_runs, side="a"),
            "base_to_one_runs": len(base_to_one_runs),
            "base_to_one_changed_bytes_in_base_space": total_changed_bytes(base_to_one_runs, side="a"),
        },
        "candidate_filter": {
            "noise_overlap_threshold": float(args.noise_overlap_threshold),
            "insertion_tolerance": int(args.insertion_tolerance),
            "top_n": int(args.top),
            "block_size": block_size,
        },
        "candidate_counts": {
            "raw_target_runs": len(target_runs),
            "candidates_before_top": len(candidates),
            "candidates_emitted": len(candidates_sorted),
        },
    }

    (out_dir / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")

    all_fields = [
        "idx",
        "tag",
        "save_a0",
        "save_a1",
        "save_len",
        "one_b0",
        "one_b1",
        "one_len",
        "overlap_ratio_with_save_noise",
        "near_insertion_noise",
        "size_score",
        "save_ascii_context",
        "one_ascii_context",
        "save_hex_context",
        "one_hex_context",
    ]
    write_csv(out_dir / "runs_save_to_one_all.csv", all_rows, all_fields)
    write_csv(out_dir / "runs_save_to_one_candidates.csv", candidates_sorted, all_fields)

    print("Wrote:")
    print(f"  {out_dir / 'summary.json'}")
    print(f"  {out_dir / 'runs_save_to_one_all.csv'}")
    print(f"  {out_dir / 'runs_save_to_one_candidates.csv'}")
    print("")
    print(json.dumps(summary, indent=2))


if __name__ == "__main__":
    main()
