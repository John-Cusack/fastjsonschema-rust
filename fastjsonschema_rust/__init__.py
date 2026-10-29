"""fastjsonschema_rust: Rust validation kernel, upstream-compatible API."""
import os

import fastjsonschema
from fastjsonschema import (  # noqa: F401  (re-exported API surface)
    JsonSchemaException,
    JsonSchemaValueException,
    JsonSchemaValuesException,
    JsonSchemaDefinitionException,
    VERSION,
)
from fastjsonschema.draft04 import CodeGeneratorDraft04  # noqa: F401
from fastjsonschema.draft06 import CodeGeneratorDraft06  # noqa: F401
from fastjsonschema.draft07 import CodeGeneratorDraft07  # noqa: F401
from fastjsonschema.draft2019 import CodeGeneratorDraft2019  # noqa: F401
from fastjsonschema.ref_resolver import RefResolver  # noqa: F401
from fastjsonschema import _get_code_generator_class  # noqa: F401 (test-suite compat)

from . import _plan
from ._core import Validator  # noqa: E402

__all__ = (
    'VERSION',
    'JsonSchemaException',
    'JsonSchemaValueException',
    'JsonSchemaValuesException',
    'JsonSchemaDefinitionException',
    'validate',
    'compile',
    'compile_to_code',
)


def _translate_env_flag(name):
    return os.environ.get(name) == '1'


def compile(definition, handlers={}, formats={}, use_default=True,
            use_formats=True, detailed_exceptions=True, fast_fail=True):
    """Compile a schema to a validating function (Rust kernel)."""
    if _translate_env_flag('FASTJSONSCHEMA_RUST_FALLBACK'):
        return fastjsonschema.compile(
            definition, handlers, formats, use_default,
            use_formats, detailed_exceptions, fast_fail)
    try:
        plan, _builder = _plan.build_plan(
            definition, handlers, formats, use_default, use_formats,
            detailed_exceptions, fast_fail)
    except _plan.Unsupported:
        return fastjsonschema.compile(
            definition, handlers, formats, use_default,
            use_formats, detailed_exceptions, fast_fail)
    validator = Validator(plan)
    def validate_fn(data, custom_formats=None, name_prefix=None):
        if custom_formats is None:
            custom_formats = formats
        return validator.validate(data, custom_formats, name_prefix)
    validate_fn.__name__ = 'validate'
    validate_fn.validate_many = validator.validate_many
    validate_fn.validator = validator
    return validate_fn
def validate(definition, data, handlers={}, formats={}, use_default=True,
             use_formats=True, detailed_exceptions=True, fast_fail=True):
    return compile(definition, handlers, formats, use_default,
                   use_formats, detailed_exceptions, fast_fail)(data)


def compile_to_code(definition, handlers={}, formats={}, use_default=True,
                    use_formats=True, detailed_exceptions=True,
                    fast_fail=True):
    """Code generation stays in Python (upstream implementation)."""
    return fastjsonschema.compile_to_code(
        definition, handlers, formats, use_default,
        use_formats, detailed_exceptions, fast_fail)
