#!/usr/bin/env python3
"""
ECT M2.1 Variant Pack Builder

One-command generation of multiple offline variants for fast DAW A/B:

1. NEUTRAL (optional)
2. LONG bias
3. SHORT bias
4. REP bias

Outputs per-variant artifact folders and an optional bundled multi-track MIDI.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path
from typing import Dict, List, Optional, Sequence, Tuple

import mido


REPO_ROOT = Path(__file__).resolve().parents[1]
ECT_CORE_CLI = REPO_ROOT / "core" / "ect_core_cli.py"


def parse_biases(raw: str) -> List[str]:
    vals = [x.strip().upper() for x in raw.split(",") if x.strip()]
    valid = {"LONG", "SHORT", "REP"}
    out: List[str] = []
    for v in vals:
        if v not in valid:
            raise ValueError(f"Invalid bias '{v}'. Allowed: LONG,SHORT,REP")
        if v not in out:
            out.append(v)
    return out


def run_variant(
    python_exe: str,
    source_midi: Path,
    source_audio: Path,
    track_index: int,
    config: Optional[Path],
    articulations: Optional[Path],
    output_dir: Path,
    run_id: str,
    region_start_tick: Optional[int],
    region_end_tick: Optional[int],
    bias: Optional[str],
) -> None:
    cmd = [
        python_exe,
        str(ECT_CORE_CLI),
        "--source-midi",
        str(source_midi),
        "--source-audio",
        str(source_audio),
        "--track-index",
        str(track_index),
        "--output-dir",
        str(output_dir),
        "--run-id",
        run_id,
    ]
    if config is not None:
        cmd.extend(["--config", str(config)])
    if articulations is not None:
        cmd.extend(["--articulations", str(articulations)])
    if region_start_tick is not None and region_end_tick is not None:
        cmd.extend(["--region-start-tick", str(region_start_tick), "--region-end-tick", str(region_end_tick)])
    if bias is not None:
        cmd.extend(["--bias-profile", bias])

    subprocess.run(cmd, check=True)


def load_manifest(path: Path) -> Dict:
    return json.loads(path.read_text(encoding="utf-8"))


def bundle_variants_midi(output_path: Path, variants: Sequence[Tuple[str, Path]]) -> None:
    if not variants:
        return

    mids: List[Tuple[str, mido.MidiFile]] = []
    for name, folder in variants:
        p = folder / "control_stream.mid"
        mids.append((name, mido.MidiFile(p)))

    tpq = mids[0][1].ticks_per_beat
    out = mido.MidiFile(ticks_per_beat=tpq)

    # Tempo/meta track from the first variant.
    tempo_src = mids[0][1].tracks[0]
    tempo_track = mido.MidiTrack()
    out.tracks.append(tempo_track)
    for msg in tempo_src:
        tempo_track.append(msg.copy(time=msg.time))

    # One CC track per variant.
    for name, mid in mids:
        src_cc_track = mid.tracks[1] if len(mid.tracks) > 1 else mido.MidiTrack()
        tr = mido.MidiTrack()
        out.tracks.append(tr)
        tr.append(mido.MetaMessage("track_name", name=f"ECT_{name}_CC", time=0))
        for msg in src_cc_track:
            tr.append(msg.copy(time=msg.time))

    output_path.parent.mkdir(parents=True, exist_ok=True)
    out.save(output_path)


def main() -> int:
    ap = argparse.ArgumentParser(description="Build NEUTRAL + bias variant pack for quick DAW A/B")
    ap.add_argument("--source-midi", required=True, type=Path)
    ap.add_argument("--source-audio", required=True, type=Path)
    ap.add_argument("--track-index", type=int, default=1)
    ap.add_argument("--config", type=Path, default=Path("/Users/sk/ECT/contracts/config.template.yaml"))
    ap.add_argument(
        "--articulations",
        type=Path,
        default=Path("/Users/sk/MAXMSP Patches/Synchron_MFT_Master 2_WORK/articulations.json"),
    )
    ap.add_argument("--output-dir", required=True, type=Path)
    ap.add_argument("--run-prefix", type=str, default="variantpack")
    ap.add_argument("--biases", type=str, default="LONG,SHORT,REP")
    ap.add_argument("--no-neutral", action="store_true", help="Skip neutral baseline render")
    ap.add_argument("--region-start-tick", type=int)
    ap.add_argument("--region-end-tick", type=int)
    ap.add_argument("--bundle-midi", action="store_true", help="Emit single multi-track MIDI bundle")
    args = ap.parse_args()

    if not args.source_midi.exists():
        raise FileNotFoundError(f"Missing source MIDI: {args.source_midi}")
    if not args.source_audio.exists():
        raise FileNotFoundError(f"Missing source audio: {args.source_audio}")

    if (args.region_start_tick is None) ^ (args.region_end_tick is None):
        raise ValueError("Provide both --region-start-tick and --region-end-tick together.")
    if (
        args.region_start_tick is not None
        and args.region_end_tick is not None
        and args.region_end_tick < args.region_start_tick
    ):
        raise ValueError("Region end tick must be >= region start tick.")

    output_dir = args.output_dir
    output_dir.mkdir(parents=True, exist_ok=True)
    biases = parse_biases(args.biases)

    variants_to_run: List[Tuple[str, Optional[str]]] = []
    if not args.no_neutral:
        variants_to_run.append(("NEUTRAL", None))
    for b in biases:
        variants_to_run.append((b, b))

    python_exe = sys.executable
    summary: Dict[str, Dict] = {}
    variant_folders: List[Tuple[str, Path]] = []

    for name, bias in variants_to_run:
        folder = output_dir / name.lower()
        run_id = f"{args.run_prefix}-{name.lower()}"
        run_variant(
            python_exe=python_exe,
            source_midi=args.source_midi,
            source_audio=args.source_audio,
            track_index=args.track_index,
            config=args.config if args.config and args.config.exists() else None,
            articulations=args.articulations if args.articulations and args.articulations.exists() else None,
            output_dir=folder,
            run_id=run_id,
            region_start_tick=args.region_start_tick,
            region_end_tick=args.region_end_tick,
            bias=bias,
        )
        manifest = load_manifest(folder / "run_manifest.json")
        metrics = json.loads((folder / "metrics.json").read_text(encoding="utf-8"))
        summary[name] = {
            "folder": str(folder),
            "run_id": manifest.get("run_id"),
            "overall_pass": metrics.get("gate_results", {}).get("overall_pass"),
            "cc1_mean": metrics.get("cc_stats", {}).get("cc1", {}).get("mean"),
            "cc11_mean": metrics.get("cc_stats", {}).get("cc11", {}).get("mean"),
        }
        variant_folders.append((name, folder))

    bundle_path = None
    if args.bundle_midi:
        bundle_path = output_dir / "control_stream_variants.mid"
        bundle_variants_midi(bundle_path, variant_folders)

    pack_manifest = {
        "run_prefix": args.run_prefix,
        "source_midi": str(args.source_midi),
        "source_audio": str(args.source_audio),
        "track_index": args.track_index,
        "region_start_tick": args.region_start_tick,
        "region_end_tick": args.region_end_tick,
        "variants": summary,
        "bundle_midi": str(bundle_path) if bundle_path else None,
    }
    (output_dir / "variant_pack_manifest.json").write_text(json.dumps(pack_manifest, indent=2), encoding="utf-8")

    print(f"Variant pack complete: {output_dir}")
    for name in summary.keys():
        print(f"- {name}: {summary[name]['folder']}")
    if bundle_path:
        print(f"- bundled MIDI: {bundle_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

