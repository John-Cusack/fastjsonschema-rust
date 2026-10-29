"""Tier-1 driver: min-of-7 fresh-subprocess walls per (arm, track).

Checks outcome checksums + error samples agree across arms within a
track, prints a labeled bench table.
"""
import json
import subprocess
import sys

VENV_PY = "/home/john/firstmate/data/rs-port-fastjsonschema/scratch/venv/bin/python"
WORKER = "/home/john/firstmate/data/rs-port-fastjsonschema/scratch/bench_worker.py"

ARMS = ["base", "armB", "armC", "armD", "armE"]
TRACKS = ["rep", "valid", "x1"]
RUNS = 7


def one(arm, track):
    out = subprocess.run([VENV_PY, WORKER, arm, track],
                         capture_output=True, text=True, check=True)
    return json.loads(out.stdout)


def main():
    rows = {}
    for track in TRACKS:
        for arm in ARMS:
            walls, peaks, rec = [], [], None
            for _ in range(RUNS):
                rec = one(arm, track)
                walls.append(rec["wall"])
                peaks.append(rec["peak_kb"])
            rows[(arm, track)] = {
                "wall": min(walls), "walls": walls,
                "peak_kb": min(peaks), "ok": rec["ok"],
                "repeat": rec["repeat"],
                "checksum": rec["checksum"], "sample": rec["sample"],
            }

    # Equivalence: checksums + error samples must match base within track.
    for track in TRACKS:
        base = rows[("base", track)]
        for arm in ARMS[1:]:
            r = rows[(arm, track)]
            assert r["checksum"] == base["checksum"], (arm, track, "checksum")
            assert r["ok"] == base["ok"], (arm, track, "ok")
            assert r["sample"] == base["sample"], (arm, track, "sample")
    print("EQUIV OK: checksums + error samples agree on all tracks/arms")

    for track in TRACKS:
        print(f"\n== {track} (work per iter: "
              f"{rows[('base', track)]['repeat']} passes) ==")
        b = rows[("base", track)]["wall"]
        for arm in ARMS:
            r = rows[(arm, track)]
            print(f"  {arm:6s} wall={r['wall']:.4f}s "
                  f"({b / r['wall']:.3f}x vs base) peak={r['peak_kb']} KiB")

    with open("/home/john/firstmate/data/rs-port-fastjsonschema/bench_tier1_results.json", "w") as f:
        json.dump({f"{a}/{t}": {k: v for k, v in rows[(a, t)].items()
                                if k != "sample"}
                   for a in ARMS for t in TRACKS}, f, indent=1)


main()
