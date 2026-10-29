# Plan IR spec (fastjsonschema-rust)

Per-schema check plan built once in Python (`fastjsonschema_rust/_plan.py`),
executed by the Rust kernel. Mirrors upstream codegen structure so error
messages, paths, rules, definitions, and check ORDER are identical.

## Shape

```python
plan = {
    "draft": "04" | "06" | "07" | "2019",
    "options": {"use_default": bool, "use_formats": bool,
                "detailed": bool, "fast_fail": bool},
    "levels": [level, ...],   # index 0 is the root
    "regexps": [...],          # opaque compiled re.Pattern, by index
    "calls": [...],            # opaque custom-format callables, by index
}
level = {"checks": [check, ...], "d": <expanded definition pyobj>}
check = {"k": <keyword>, "e": [before, after] | None, "r": rule | None,
         ...params}
```

- `"d"` is `repr_default(resolver-expanded definition)` computed with the
  upstream generator's own helpers: the exact object embedded in errors.
- `"e": [before, after]` are the static message parts around `{name}`;
  `message = before + name + after + dynamic`. `None` for checks that
  never fail (defaults are applied by `"k": "default"` pseudo-checks).
- `"dyn"` on a check names the runtime-computed tail: `missing`
  (`str(sorted(missing)) + " properties"`), `extraset`
  (`str(extra_keys_set) + " properties"`), `oneof`
  (`" (N matches found)"`), else none.
- Check order inside a level == upstream `_json_keywords_to_function`
  dispatch order (dependencies before properties; contentEncoding /
  contentMediaType last). Nested levels carry their own order.
- `$ref` becomes `{"k": "ref", "target": idx, "pinned_fast": bool}`.
  `pinned_fast` replicates upstream's lazy codegen: a URI first
  generated under a trial block keeps its fast-fail body even when the
  compile is collect-mode (single ValueException aborts collection).
- Recursive refs are `ref` cycles; the kernel guards depth.
- `pattern` / `format` regexes are opaque `re.Pattern` objects called
  from Rust (identical `re` semantics by construction); custom-format
  callables are invoked per value (same cost as upstream for that node).
- Unknown keywords are ignored (upstream ignores them too).
- Anything the planner cannot express raises `Unsupported` and the
  public `compile()` falls back to pure-Python upstream compile.

## Numeric / structural semantics

Performed through the C API on the live Python objects (`RichCompare`,
real `set`/`list`/`Decimal` ops), never reimplemented: mixed
int/float/Decimal comparisons, `multipleOf` Decimal dance (+ the
`inifinity reached` typo), `uniqueItems` normalization, `sorted()` /
`str(set)` dynamic message parts. `str` vs `repr` key order hazards
(`extraset`) use real `PySet` construction in upstream order, so
in-process messages agree; cross-process set order is hash-seed
dependent upstream too (documented residual, fixed-seed corpus).
Brace-containing property keys are plain-concatenated (upstream
crashes with KeyError there; deliberate deviation, see ADR).
