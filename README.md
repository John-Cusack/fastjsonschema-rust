# fastjsonschema-rust

Rust-accelerated JSON Schema validation kernel for
[horejsek/python-fastjsonschema](https://github.com/horejsek/python-fastjsonschema)
(experimental port lane). Per-schema check plans compiled once in
Python, executed by a Rust validation kernel with whole-batch
crossing; `format`/`pattern` regexes and custom format callables run
as opaque Python nodes with identical semantics. `compile` /
`compile_to_code` API stays Python; importable pure-Python fallback.

See [NOTICE](NOTICE) for upstream attribution (BSD-3-Clause).
Ported against fastjsonschema 2.22.2.

## Layout

- `fastjsonschema-core/` — plan IR + validation engine (Rust).
- `src/` — PyO3 bindings (`_core` extension module).
- `fastjsonschema_rust/` — Python package (`compile`, `validate`,
  plan translator `_plan.py`, fallback).
- `tests/` — upstream-suite gate + equivalence corpus + benches.
- `specs/` — plan IR spec.
