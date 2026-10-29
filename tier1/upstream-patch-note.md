# Upstream patch note — faster generated-code shapes (no PR opened)

Against horejsek/python-fastjsonschema @ 738da70 (clone HEAD; the
shapes patched are identical in the 2.22.2 sdist).

## What

Three pure-Python codegen-shape changes to the generated `validate`
functions (see `tier1.patch` in this lane dir):

1. **required without set ops** (`draft04.generate_required`): emit
   `[k for k in [...] if k not in data]` instead of
   `set([...]) - data.keys()`. Same exception type, message
   (`str(sorted(missing))`), `definition`, and `rule`; the set
   construction moves off the valid path.
2. **no dead `_keys` set** (`generate_properties` /
   `generate_pattern_properties` / `generate_additional_properties`):
   skip building, probing (`in`), and pruning (`.remove`) the
   `{var}_keys` set when no consumer exists at that object level
   (`additionalProperties` absent/`True`/`{}` and no
   `propertyNames: false`); direct `if "key" in {variable}` guards
   instead.
3. **enum tuple membership** (`generate_enum`): for all-string enums
   emit `if x not in ('a', 'b', ...)` instead of the per-option
   `isinstance-and-==` or-chain. Same message, `definition`, `rule`.

## Measured (Ryzen 9 5950X, CPython 3.12.3, min-of-7 fresh-subprocess
## walls, frozen WORKLOAD.md v1, 3000-doc nested order batch)

| arm | rep (half-invalid) | valid-only | x1 (single small doc) |
|---|---|---|---|
| stock | 0.3571 s | 0.2825 s | 0.0145 s |
| required-only | 1.21x | 1.31x | 1.41x |
| keys-only | 1.14x | 1.20x | 1.17x |
| enum-only | 1.11x | 0.99x | 1.03x |
| combined | **1.63x** | **1.76x** | **1.98x** |

Peak RSS flat across arms (20–22 MiB). Outcome checksums and error
samples (message, name, rule, definition) agree on all tracks/arms.

## Why upstream-plausible

All three are generated-code shapes any consumer of the library
benefits from with no API change; semantics (including exception
texts/paths) are byte-identical by construction and verified by the
equivalence check above.
