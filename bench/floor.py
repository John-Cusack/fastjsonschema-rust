"""Rewrite-ceiling floor: irreducible costs a Rust kernel still pays.

A Rust validation kernel must (a) touch every validated field through
the C API and (b) build + raise identical Python exceptions on the
failure path. This measures both floors in CPython (the traversal
loop overestimates Rust's C-API walk, so the resulting ceiling is
conservative) and computes ceiling = E_rep / floor_total.

Usage: venv python floor.py  (prints labeled numbers)
"""
import sys
import time

sys.path.insert(0, "/home/john/firstmate/data/rs-port-fastjsonschema/scratch/fjs_armE")
import fastjsonschema  # noqa: E402
from fastjsonschema import JsonSchemaException, JsonSchemaValueException  # noqa: E402

sys.path.insert(0, "/home/john/firstmate/data/rs-port-fastjsonschema/scratch")
sys.argv = ["bench_worker.py", "armE", "rep"]
# Corpus builders replicated here (kept in sync with WORKLOAD.md).
import hashlib  # noqa: E402

NDOCS = 3000


def make_doc(i, valid):
    doc = {"id": f"ord-{i}", "total": (i * 37) % 5000,
           "status": ["new", "paid", "shipped", "cancelled"][i % 4],
           "customer": {"name": f"c-{i % 90}", "vip": bool(i % 3),
                        "tags": ["a", "b"]},
           "items": [{"sku": f"s-{k}", "qty": 1 + (k + i) % 4,
                      "price": 9.5 + k} for k in range(3)]}
    if not valid:
        doc["items"][0]["qty"] = "two"
    return doc


DOCS = [make_doc(i, valid=(i % 2 == 0)) for i in range(NDOCS)]
REPEAT = 20


def traversal():
    """Touch every validated field; no checks. Upper-bounds Rust's walk."""
    acc = 0
    for d in DOCS:
        acc += len(d["id"]) + (d["total"] >= 0)
        acc += (d["status"] == "new") + len(d["customer"]["name"])
        acc += d["customer"]["vip"] + len(d["customer"]["tags"])
        for it in d["items"]:
            acc += len(it["sku"]) + (it["qty"] if isinstance(it["qty"], int) else 0) + (it["price"] >= 0)
    return acc


def exc_cost():
    """Raise + catch the identical exception shape the kernel must build."""
    n = 0
    for d in DOCS:
        if d["items"][0]["qty"] == "two":
            try:
                v = d["items"][0]["qty"]
                raise JsonSchemaValueException(
                    "data.items[0].qty must be integer", value=v,
                    name="data.items[0].qty",
                    definition={'type': 'integer', 'minimum': 1}, rule='type')
            except JsonSchemaException:
                n += 1
    return n


def timeit(fn, repeat):
    fn()
    t0 = time.perf_counter()
    for _ in range(repeat):
        fn()
    return time.perf_counter() - t0


def main():
    t_trav = timeit(traversal, REPEAT)
    # exceptions fire on half the docs per pass; exc_cost covers one pass
    t_exc = timeit(exc_cost, REPEAT)
    print(f"traversal-only {REPEAT} passes: {t_trav:.4f}s")
    print(f"identical-exceptions {REPEAT} passes: {t_exc:.4f}s")
    floor = t_trav + t_exc
    print(f"floor total: {floor:.4f}s")
    # E_rep wall loaded from the tier-1 bench results.
    import json
    res = json.load(open("/home/john/firstmate/data/rs-port-fastjsonschema/bench_tier1_results.json"))
    e_rep = res["armE/rep"]["wall"]
    print(f"E rep wall: {e_rep:.4f}s  conservative ceiling: {e_rep / floor:.2f}x")


main()
