"""Equivalence corpus: stock fastjsonschema vs fastjsonschema_rust.

Seeded differential over keyword families x drafts x error modes.
Compares outcomes, return values (incl. default mutation), and full
error shape (type, message, name, path, value, rule, definition).
Run with a fixed hash seed; set-order-sensitive messages agree
in-process by construction (identical set build order).
"""
import copy
import itertools
import os

import pytest

import fastjsonschema as STOCK
import fastjsonschema_rust as PORT

HASH_SEED = os.environ.get("PYTHONHASHSEED")
assert HASH_SEED is not None, "run with PYTHONHASHSEED fixed"

FORMATS = {
    "even": lambda v: v % 2 == 0,
    "lower": r"^[a-z]+$",
}


def schemas():
    S = []
    add = S.append
    # scalars x drafts
    for draft in ("04", "06", "07", "2019"):
        base = {"$schema": f"http://json-schema.org/draft-{draft}/schema"}
        add((dict(base, type="string", minLength=2, maxLength=5,
                  pattern="^a+$", format="lower"), ["aab", "x", 1, None]))
        add((dict(base, type="integer", minimum=3, maximum=9), [3, 9, 2, 10, 3.5, True]))
        add((dict(base, type="number", multipleOf=0.5,
                  exclusiveMinimum=1, exclusiveMaximum=9), [1.5, 1, 9, 0.5, "x"]))
        add((dict(base, type="string", format="even"), ["a", 1]))
        add((dict(base, enum=[1, "a", None, True, [1], {"k": 1}]),
             [1, "a", None, True, [1], {"k": 1}, 2, [2], {"k": 2}]))
        add((dict(base, const={"a": [1, 2]}),
             [{"a": [1, 2]}, {"a": [2, 1]}, {"a": [1]}, 1]))
    # objects
    add(({"type": "object", "required": ["a", "b"],
          "properties": {"a": {"type": "string"}, "b": {"type": "integer", "default": 7}},
          "additionalProperties": False},
         [{"a": "x", "b": 1}, {"a": "x"}, {}, {"a": "x", "b": 1, "c": 2},
          {"a": "x", "b": "y"}]))
    add(({"type": "object", "properties": {"a-b": {"type": "string"}},
          "patternProperties": {"^x": {"type": "number"}},
          "additionalProperties": {"type": "boolean"}},
         [{"a-b": "s", "x1": 1, "z": True}, {"a-b": "s", "x1": "s"},
          {"a-b": "s", "z": 1}, {}]))
    add(({"type": "object", "dependencies": {"a": ["b"], "c": False,
                                            "d": {"type": "string"}}},
         [{"a": 1, "b": 2}, {"a": 1}, {"c": 1}, {"d": "s"}, {"d": 1}, {}]))
    add(({"propertyNames": {"maxLength": 2}}, [{"ab": 1}, {"abc": 1}, {}, "x"]))
    add(({"propertyNames": False}, [{}, {"a": 1}]))
    # arrays
    add(({"type": "array", "items": [{"type": "integer"}, {"type": "string"}],
          "additionalItems": False, "minItems": 1, "maxItems": 3,
          "uniqueItems": True},
         [[1, "a"], [1], [1, "a", "b"], [1, 1], [1, "a", "a"], [], "x"]))
    add(({"type": "array", "items": {"type": "integer"},
          "contains": {"minimum": 5}},
         [[1, 5], [1, 2], [], "x"]))
    add(({"type": "array", "items": [{"type": "string"}],
          "additionalItems": {"type": "number"}},
         [["a", 1], ["a", "b"], ["a", 1, 2]]))
    # combinators
    add(({"anyOf": [{"type": "string"}, {"type": "integer", "minimum": 5}]},
         ["a", 7, 3, None]))
    add(({"oneOf": [{"type": "number", "multipleOf": 3},
                    {"type": "number", "multipleOf": 5}]},
         [3, 5, 15, 7, "x"]))
    add(({"not": {"type": "string"}}, [1, "a", None]))
    add(({"allOf": [{"type": "number"}, {"minimum": 2}]}, [3, 1, "x"]))
    add(({"if": {"type": "integer"}, "then": {"minimum": 0},
          "else": {"type": "string", "minLength": 2}},
         [5, -1, "ab", "a", None]))
    # refs incl. recursion
    add(({"definitions": {"s": {"type": "string"}},
          "properties": {"a": {"$ref": "#/definitions/s"}}},
         [{"a": "x"}, {"a": 1}, {}]))
    tree = {"type": "object",
            "properties": {"v": {"type": "integer"},
                           "kids": {"type": "array",
                                    "items": {"$ref": "#"}}}}
    add((tree, [{"v": 1, "kids": [{"v": 2, "kids": []}]},
                {"v": 1, "kids": [{"v": "x", "kids": []}]}, {"v": "x"}]))
    add(({"type": "object",
          "properties": {"a": {"$ref": "#/definitions/a"}},
          "definitions": {"a": {"type": ["integer", "array"],
                                "items": {"$ref": "#/definitions/a"}}}},
         [{"a": [1, [2, []]]}, {"a": [1, ["x"]]}]))
    # formats + custom formats
    add(({"type": "string", "format": "email"},
         ["a@b.com", "nope", 1]))
    add(({"type": "string", "format": "even"}, ["a", 1]))
    add(({"type": "string", "format": "date-time"},
         ["2020-01-01T00:00:00Z", "yesterday", 1]))
    # content*, defaults, brace/unicode keys
    add(({"type": "string", "contentEncoding": "base64"},
         ["aGVsbG8=", "!!!", 1]))
    add(({"type": "string", "contentMediaType": "application/json"},
         ['{"a": 1}', 'nope', b'{"b": 2}', b'\xff', 1]))
    add(({"type": "object", "properties": {"{a}": {"type": "string"},
                                           "caf\u00e9": {"type": "integer"}}},
         [{"{a}": "x", "caf\u00e9": 1}, {"{a}": 1}, {}]))
    # boolean schemas (06+)
    add(({"$schema": "http://json-schema.org/draft-07/schema#",
          "properties": {"a": True, "b": False}}, [{"a": 1}, {"b": 1}, {}]))
    add(({"$schema": "http://json-schema.org/draft-07/schema#",
          "items": False}, [[], [1], "x"]))
    return S


def norm_exc(e):
    if e is None:
        return None
    if isinstance(e, STOCK.JsonSchemaValuesException):
        return ("values", e.message,
                [norm_exc(i) for i in e.errors])
    if isinstance(e, STOCK.JsonSchemaValueException):
        return ("value", e.message, e.name, e.path, e.value, e.rule,
                e.definition, e.rule_definition)
    return ("raw", type(e).__name__, str(e))


@pytest.mark.parametrize("mode", list(itertools.product(
    [True, False], [True, False])))
@pytest.mark.parametrize("schema,docs", schemas())
def test_equiv(schema, docs, mode):
    fast_fail, detailed = mode
    kw = dict(detailed_exceptions=detailed, fast_fail=fast_fail)
    try:
        a = STOCK.compile(schema, formats=FORMATS, **kw)
        compile_err_a = None
    except Exception as e:
        compile_err_a = e
    try:
        b = PORT.compile(schema, formats=FORMATS, **kw)
        compile_err_b = None
    except Exception as e:
        compile_err_b = e
    assert norm_exc(compile_err_a) == norm_exc(compile_err_b), (schema, mode)
    if compile_err_a is not None:
        return
    for doc in docs:
        da, db = copy.deepcopy(doc), copy.deepcopy(doc)
        try:
            ra = a(da)
            ea = None
        except Exception as e:
            ra, ea = None, e
        try:
            rb = b(db)
            eb = None
        except Exception as e:
            rb, eb = None, e
        assert norm_exc(ea) == norm_exc(eb), (schema, doc, mode)
        if ea is None:
            assert ra == rb == db or ra == rb, (schema, doc, mode)
            assert da == db, (schema, doc, mode)


def test_fallback_env(monkeypatch):
    monkeypatch.setenv("FASTJSONSCHEMA_RUST_FALLBACK", "1")
    v = PORT.compile({"type": "string"})
    assert v("x") == "x"
    with pytest.raises(STOCK.JsonSchemaException):
        v(1)


def test_validate_many():
    v = PORT.compile({"type": "integer"})
    assert v.validate_many([1, 2, 3]) == 3
    with pytest.raises(STOCK.JsonSchemaException):
        v.validate_many([1, "x"])


def test_name_prefix():
    v = PORT.compile({"type": "string"})
    with pytest.raises(STOCK.JsonSchemaValueException) as e:
        v(1, name_prefix="item")
    assert e.value.name == "item"
    assert "item must be string" in e.value.message
