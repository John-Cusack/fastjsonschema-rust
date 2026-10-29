# Bench rows — fastjsonschema-rust vs stock

Box: Ryzen 9 5950X, CPython 3.12.3, release extension build.
Workload: frozen `WORKLOAD.md` v1 lane record (3000-doc nested order
batch; `rep` half-valid/half-invalid, `valid` all-valid, `x1` single
small doc, `batch` one `validate_many` crossing over the 3000 valid
docs). Method: min-of-7 fresh-subprocess walls + peak self RSS both
arms; outcome checksums agree within track.

Oracle: horejsek/python-fastjsonschema @ 738da70. Port at HEAD (see
repo log). Tier-1 row = combined generated-code patch
(`upstream-patch-note.md`, same shapes, no upstream PR).

| track | stock wall | tier-1 wall | port wall | port vs stock | port vs tier-1 | peak RSS stock → port |
|---|---|---|---|---|---|---|
| rep (representative) | 0.3465 s | 0.2394 s (1.45x) | 0.1418 s | **2.44x** | **1.69x** | 25840 → 25516 KiB |
| valid | 0.2921 s | 0.1685 s (1.73x) | 0.1169 s | **2.50x** | **1.44x** | 25600 → 25312 KiB |
| x1 (overhead) | 0.0165 s | 0.0094 s (1.77x) | 0.0104 s | 1.58x | 0.90x | 20112 → 21272 KiB |
| batch (showcase) | — (loop 0.1169 s) | — | 0.1098 s | — | — | 25324 KiB |

Notes:

- The kill criterion (<1.5x over the tier-1 baseline on batched
  validate) clears at 1.69x rep (2.44x over stock).
- x1 is the per-call crossing floor: one 3-field doc per call is
  dispatch-bound on both sides; the port still wins 1.58x over stock.
  The tier-1 shape wins x1 outright (1.77x) — generated-code shapes
  dominate at tiny sizes, the kernel at batch sizes.
- `batch` (one `validate_many` crossing for the whole 3000-doc list)
  beats the per-doc loop ~6%; the representative track stays per-doc
  because that is the API shape users call.
- Peak RSS flat across arms (no allocation regression; extension pages
  only).
