"""Two-track bench: stock vs tier-1 vs port on the frozen workload.

Each (impl, track) runs in a fresh subprocess: min-of-7 wall +
peak self RSS, outcome checksums. Tracks: rep / valid / x1 per-doc,
plus batch (validate_many single crossing vs stock loop).
"""
import json
import subprocess

VENV_PY = "/home/john/firstmate/data/rs-port-fastjsonschema/scratch/devvenv/bin/python"

WORKER = '''
import hashlib, json, resource, sys, time
impl = sys.argv[1]; track = sys.argv[2]
sys.path.insert(0, "/home/john/firstmate/data/rs-port-fastjsonschema/scratch/port")
if impl == "stock":
    import fastjsonschema as F
    compile = F.compile
elif impl == "tier1":
    sys.path.insert(0, "/home/john/firstmate/data/rs-port-fastjsonschema/scratch/fjs_armE")
    import fastjsonschema as F
    compile = F.compile
else:
    import fastjsonschema_rust as F
    compile = F.compile

SCHEMA = {
    "type": "object",
    "properties": {
        "id": {"type": "string", "minLength": 3},
        "total": {"type": "number", "minimum": 0, "maximum": 100000},
        "status": {"type": "string",
                   "enum": ["new", "paid", "shipped", "cancelled"]},
        "customer": {
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "vip": {"type": "boolean"},
                "tags": {"type": "array", "items": {"type": "string"}},
            },
            "required": ["name", "vip"],
        },
        "items": {
            "type": "array", "minItems": 1,
            "items": {
                "type": "object",
                "properties": {
                    "sku": {"type": "string"},
                    "qty": {"type": "integer", "minimum": 1},
                    "price": {"type": "number", "minimum": 0},
                },
                "required": ["sku", "qty", "price"],
            },
        },
    },
    "required": ["id", "total", "status", "customer", "items"],
}
X1_SCHEMA = {"type": "object", "properties": {"a": {"type": "string"}}, "required": ["a"]}
NDOCS = 3000

def make_doc(i, valid):
    doc = {"id": "ord-%d" % i, "total": (i * 37) % 5000,
           "status": ["new", "paid", "shipped", "cancelled"][i % 4],
           "customer": {"name": "c-%d" % (i % 90), "vip": bool(i % 3), "tags": ["a", "b"]},
           "items": [{"sku": "s-%d" % k, "qty": 1 + (k + i) % 4, "price": 9.5 + k} for k in range(3)]}
    if not valid:
        doc["items"][0]["qty"] = "two"
    return doc

if track == "x1":
    validate = compile(X1_SCHEMA)
    docs = [{"a": "hello"}]
    repeat = 20000
else:
    validate = compile(SCHEMA)
    docs = [make_doc(i, valid=(track in ("valid", "batch") or i % 2 == 0)) for i in range(NDOCS)]
    repeat = 20

for d in docs:
    try:
        validate(d)
    except Exception:
        pass

t0 = time.perf_counter()
if track == "batch":
    from fastjsonschema_rust._core import Validator
    from fastjsonschema_rust import _plan as _P
    plan, _ = _P.build_plan(SCHEMA, {}, {}, True, True, True, True)
    V = Validator(plan)
    for _ in range(repeat):
        ok = V.validate_many(docs)
        h = hashlib.sha256(("batch:%d" % ok).encode())
else:
    for _ in range(repeat):
        ok = 0
        h = hashlib.sha256()
        for d in docs:
            try:
                validate(d)
                ok += 1
                h.update(b"\\x01")
            except Exception:
                h.update(b"\\x00")
wall = time.perf_counter() - t0
print(json.dumps({"impl": impl, "track": track, "wall": wall, "ok": ok,
                  "checksum": h.hexdigest(),
                  "peak_kb": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss}))
'''

IMPLS = ["stock", "tier1", "port"]
TRACKS = ["rep", "valid", "x1", "batch"]
RUNS = 7


def one(impl, track):
    with open("/tmp/bench_port_worker.py", "w") as f:
        f.write(WORKER)
    out = subprocess.run([VENV_PY, "/tmp/bench_port_worker.py", impl, track],
                         capture_output=True, text=True, check=True)
    return json.loads(out.stdout)


def main():
    rows = {}
    for track in TRACKS:
        for impl in IMPLS:
            if track == "batch" and impl != "port":
                continue
            walls, peaks, rec = [], [], None
            for _ in range(RUNS):
                rec = one(impl, track)
                walls.append(rec["wall"])
                peaks.append(rec["peak_kb"])
            rows[(impl, track)] = {"wall": min(walls), "walls": walls,
                                   "peak_kb": min(peaks), "ok": rec["ok"],
                                   "checksum": rec["checksum"]}
    for track in TRACKS:
        if track == "batch":
            r = rows[("port", "batch")]
            print(f"  port  /batch wall={r['wall']:.4f}s peak={r['peak_kb']} KiB ok={r['ok']}")
            continue
        base = rows[("stock", track)]["wall"]
        for impl in IMPLS:
            r = rows[(impl, track)]
            print(f"  {impl:6s}/{track:5s} wall={r['wall']:.4f}s "
                  f"({base / r['wall']:.3f}x vs stock) peak={r['peak_kb']} KiB ok={r['ok']}")
    for track in ("rep", "valid", "x1"):
        sums = {rows[(i, track)]["checksum"] for i in IMPLS}
        assert len(sums) == 1, (track, sums)
    assert rows[("port", "batch")]["ok"] == rows[("port", "valid")]["ok"]
    print("CHECKSUMS OK")
    with open("/home/john/firstmate/data/rs-port-fastjsonschema/bench_port_results.json", "w") as f:
        json.dump({f"{a}/{t}": v for (a, t), v in rows.items()}, f, indent=1)


main()
