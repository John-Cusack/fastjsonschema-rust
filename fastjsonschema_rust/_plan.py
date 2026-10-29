"""Schema -> check-plan translator for fastjsonschema-rust.

Builds the plan IR (see specs/plan.md) by walking the schema with the
upstream resolver, replicating upstream codegen's keyword dispatch
order, compile-time DefinitionExceptions, and error templates.
Anything inexpressible raises Unsupported -> pure-Python fallback.
"""
import re

from fastjsonschema.draft04 import (
    CodeGeneratorDraft04,
    JSON_TYPE_TO_PYTHON_TYPE,
    DOLLAR_FINDER,
)
from fastjsonschema.draft06 import CodeGeneratorDraft06
from fastjsonschema.draft07 import CodeGeneratorDraft07
from fastjsonschema.draft2019 import CodeGeneratorDraft2019
from fastjsonschema.exceptions import JsonSchemaDefinitionException
from fastjsonschema.generator import enforce_list
from fastjsonschema.ref_resolver import RefResolver


class Unsupported(Exception):
    """Raised when the schema needs the pure-Python fallback."""


def select_class(schema):
    if isinstance(schema, dict):
        v = schema.get('$schema', '')
        if 'draft-04' in v:
            return CodeGeneratorDraft04, '04'
        if 'draft-06' in v:
            return CodeGeneratorDraft06, '06'
        if 'draft-07' in v:
            return CodeGeneratorDraft07, '07'
        if 'draft/2019' in v or 'draft-2019' in v:
            return CodeGeneratorDraft2019, '2019'
    return CodeGeneratorDraft2019, '2019'


# Draft keyword sets (upstream adds keys per draft class).
BASE_KEYWORDS = [
    'type', 'enum', 'allOf', 'anyOf', 'oneOf', 'not',
    'minLength', 'maxLength', 'pattern', 'format',
    'minimum', 'maximum', 'multipleOf',
    'minItems', 'maxItems', 'uniqueItems', 'items',
    'minProperties', 'maxProperties', 'required',
    'dependencies', 'properties', 'patternProperties',
    'additionalProperties',
]
DRAFT06_KEYWORDS = BASE_KEYWORDS + [
    'exclusiveMinimum', 'exclusiveMaximum', 'propertyNames',
    'contains', 'const',
]
DRAFT07_KEYWORDS = DRAFT06_KEYWORDS + [
    'if', 'contentEncoding', 'contentMediaType',
]
KEYWORD_SETS = {
    '04': frozenset(BASE_KEYWORDS),
    '06': frozenset(DRAFT06_KEYWORDS),
    '07': frozenset(DRAFT07_KEYWORDS),
    '2019': frozenset(DRAFT07_KEYWORDS),
}
# Dispatch order == upstream _json_keywords_to_function order.
KEYWORD_ORDER = BASE_KEYWORDS + [
    'exclusiveMinimum', 'exclusiveMaximum', 'propertyNames',
    'contains', 'const',
    'if', 'contentEncoding', 'contentMediaType',
]


class Builder:
    def __init__(self, definition, handlers, formats, use_default,
                 use_formats, detailed, fast_fail):
        cls, self.draft = select_class(definition)
        self.resolver = RefResolver.from_schema(
            definition, handlers=handlers, store={})
        # Throwaway generator: pure helpers only (_expand_refs, e,
        # FORMAT_REGEXS, _custom_formats flags).
        self.gen = cls(
            definition, resolver=self.resolver, formats=formats,
            use_default=use_default, use_formats=use_formats,
            detailed_exceptions=detailed, fast_fail=fast_fail)
        self.use_default = use_default
        self.use_formats = use_formats
        self.levels = []
        self.memo = {}    # uri -> level idx
        self.pinned = {}  # uri -> pinned_fast at first build
        self.regexps = []
        self.calls = []
        self.trial = False
        self.current_varname = 'data'

    @staticmethod
    def child_var(key):
        # Mirrors codegen's variable naming for property children.
        return re.sub(r'($[^a-zA-Z]|[^a-zA-Z0-9])', '', key)

    @staticmethod
    def skey(key):
        # Runtime check/name key: upstream embeds e(key) in generated
        # source, whose runtime value is the key itself (str) or
        # str(key) for non-str keys.
        return key if isinstance(key, str) else str(key)
    def err(self, before, after, rule, dyn=None):
        chk = {'e': [before, after], 'r': rule}
        if dyn is not None:
            chk['dyn'] = dyn
        return chk

    def expand(self, definition):
        return self.gen._expand_refs(definition)

    def rx(self, pattern):
        """Compile pattern with upstream's $ -> \\Z transform (same errors)."""
        fixed = DOLLAR_FINDER.sub(r'\\Z', pattern)
        compiled = re.compile(fixed)
        self.regexps.append(compiled)
        return len(self.regexps) - 1

    def rx_raw(self, pattern):
        compiled = re.compile(pattern)
        self.regexps.append(compiled)
        return len(self.regexps) - 1

    def call(self, fn):
        self.calls.append(fn)
        return len(self.calls) - 1

    # -- levels --------------------------------------------------
    def build(self):
        root_uri = self.resolver.get_uri()
        idx = self.new_level(root_uri, False)
        with self.resolver.resolving(root_uri) as target:
            self.fill_level(idx, target, 'data')
        return {
            'draft': self.draft,
            'options': {
                'use_default': self.use_default,
                'use_formats': self.use_formats,
                'detailed': self.gen._detailed_exceptions,
                'fast_fail': self.gen._fast_fail,
            },
            'levels': self.levels,
            'regexps': self.regexps,
            'calls': self.calls,
        }

    def new_level(self, uri, pinned):
        idx = len(self.levels)
        self.levels.append(None)
        if uri is not None:
            self.memo[uri] = idx
            self.pinned[uri] = pinned
        return idx

    def ref_level(self, ref):
        with self.resolver.in_scope(ref):
            uri = self.resolver.get_uri()
        if uri in self.memo:
            return {'k': 'ref', 'target_uri': uri,
                    'pinned_fast': self.pinned[uri]}
        # resolving(ref): single scope join. resolving(uri) would join
        # the already scope-resolved uri against the scope a second time.
        with self.resolver.resolving(ref) as target:
            idx = self.new_level(uri, self.trial)
            self.fill_level(idx, target, 'data')
        return {'k': 'ref', 'target_uri': uri,
                'pinned_fast': self.pinned[uri]}

    def sub_level(self, definition, varname):
        """Inline subschema -> level idx (bool aware per draft)."""
        if isinstance(definition, bool):
            if self.draft == '04':
                raise JsonSchemaDefinitionException(
                    'definition must be an object')
            idx = self.new_level(None, self.trial)
            if definition is False:
                self.levels[idx] = {
                    'checks': [dict({'k': 'false'},
                                    **self.err('', ' must not be there', None))],
                    'd': self.expand(definition),
                }
            else:
                self.levels[idx] = {
                    'checks': [], 'd': self.expand(definition)}
            return idx
        if not isinstance(definition, dict):
            raise JsonSchemaDefinitionException(
                'definition must be an object')
        idx = self.new_level(None, self.trial)
        if '$ref' in definition:
            # $ref overrides siblings: level holds only the ref check.
            self.levels[idx] = {
                'checks': [self.ref_level(definition['$ref'])],
                'd': self.expand(definition),
            }
            return idx
        self.fill_level(idx, definition, varname)
        return idx

    def fill_level(self, idx, definition, varname):
        if isinstance(definition, bool):
            if self.draft == '04':
                raise JsonSchemaDefinitionException(
                    'definition must be an object')
            if definition is False:
                self.levels[idx] = {
                    'checks': [dict({'k': 'false'},
                                    **self.err('', ' must not be there', None))],
                    'd': self.expand(definition),
                }
            else:
                self.levels[idx] = {'checks': [],
                                    'd': self.expand(definition)}
            return
        if not isinstance(definition, dict):
            raise JsonSchemaDefinitionException(
                'definition must be an object')
        if '$ref' in definition:
            # Ref overrides siblings: level holds only the ref check.
            self.levels[idx] = {
                'checks': [self.ref_level(definition['$ref'])],
                'd': self.expand(definition),
            }
            return
        old_var, self.current_varname = self.current_varname, varname
        try:
            checks = []
            allowed = KEYWORD_SETS[self.draft]
            for key in KEYWORD_ORDER:
                if key in definition and key in allowed:
                    fn = getattr(self, 'kw_' + key, None)
                    if fn is None:
                        continue  # unknown to this draft; ignored
                    fn(definition, checks)
            self.levels[idx] = {'checks': checks,
                                'd': self.expand(definition)}
        finally:
            self.current_varname = old_var

    # -- keyword translators (mirrors generate_*) ----------------
    def kw_type(self, d, checks):
        types = enforce_list(d['type'])
        try:
            [JSON_TYPE_TO_PYTHON_TYPE[t] for t in types]
        except KeyError as exc:
            raise JsonSchemaDefinitionException('Unknown type') from exc
        checks.append(dict({
            'k': 'type', 'types': types, 'draft': self.draft,
        }, **self.err('', ' must be ' + ' or '.join(types), 'type')))

    def kw_enum(self, d, checks):
        enum = d['enum']
        if not isinstance(enum, (list, tuple)):
            raise JsonSchemaDefinitionException('enum must be an array')
        checks.append(dict(
            {'k': 'enum', 'values': list(enum)},
            **self.err('', ' must be one of ' + str(enum), 'enum')))

    def kw_allOf(self, d, checks):
        v = self.current_varname
        checks.append({'k': 'allof',
                       'branches': [self.sub_level(b, v) for b in d['allOf']]})

    def kw_anyOf(self, d, checks):
        v = self.current_varname
        with self.trial_scope():
            branches = [self.sub_level(b, v) for b in d['anyOf']]
        checks.append(dict({'k': 'anyof', 'branches': branches},
                           **self.err('', ' cannot be validated by any definition',
                                      'anyOf')))

    def kw_oneOf(self, d, checks):
        v = self.current_varname
        with self.trial_scope():
            branches = [self.sub_level(b, v) for b in d['oneOf']]
        checks.append(dict({'k': 'oneof', 'branches': branches},
                           **self.err('', ' must be valid exactly by one definition',
                                      'oneOf', dyn='oneof')))

    def kw_not(self, d, checks):
        nd = d['not']
        if nd is True:
            checks.append(dict({'k': 'false'},
                               **self.err('', ' must not be there', 'not')))
        elif nd is False:
            return
        elif not nd:
            checks.append(dict(
                {'k': 'false'},
                **self.err('', ' must NOT match a disallowed definition', 'not')))
        else:
            with self.trial_scope():
                sub = self.sub_level(nd, self.current_varname)
            checks.append(dict(
                {'k': 'not', 'branch': sub},
                **self.err('', ' must NOT match a disallowed definition', 'not')))

    def kw_minLength(self, d, checks):
        n = d['minLength']
        if not isinstance(n, (int, float)):
            raise JsonSchemaDefinitionException('minLength must be a number')
        checks.append(dict({'k': 'minlength', 'n': n},
                           **self.err('', ' must be longer than or equal to {} characters'.format(n),
                                      'minLength')))

    def kw_maxLength(self, d, checks):
        n = d['maxLength']
        if not isinstance(n, (int, float)):
            raise JsonSchemaDefinitionException('maxLength must be a number')
        checks.append(dict({'k': 'maxlength', 'n': n},
                           **self.err('', ' must be shorter than or equal to {} characters'.format(n),
                                      'maxLength')))

    def kw_pattern(self, d, checks):
        pattern = d['pattern']
        idx = self.rx(pattern)
        checks.append(dict({'k': 'pattern', 'rx': idx},
                           **self.err('', ' must match pattern ' + pattern, 'pattern')))

    def kw_format(self, d, checks):
        if not self.use_formats:
            return
        format_ = d['format']
        customs = self.gen._custom_formats
        if format_ not in customs and format_ not in self.gen.FORMAT_REGEXS and format_ != 'regex':
            return
        if format_ in customs:
            custom = customs[format_]
            if isinstance(custom, str):
                idx = self.rx_raw(custom)
                checks.append(dict(
                    {'k': 'format_rx', 'rx': idx, 'fmt': format_},
                    **self.err('', ' must be ' + format_, 'format')))
            else:
                idx = self.call(custom)
                checks.append(dict(
                    {'k': 'format_call', 'call': idx, 'fmt': format_},
                    **self.err('', ' must be ' + format_, 'format')))
        elif format_ in self.gen.FORMAT_REGEXS:
            idx = self.rx_raw(self.gen.FORMAT_REGEXS[format_])
            checks.append(dict({'k': 'format_rx', 'rx': idx, 'fmt': format_},
                               **self.err('', ' must be ' + format_, 'format')))
        else:  # 'regex'
            checks.append(dict({'k': 'format_regex'},
                               **self.err('', ' must be a valid regex', 'format')))

    def _number(self, name, d):
        import decimal
        n = d[name]
        if not isinstance(n, (int, float, decimal.Decimal)):
            raise JsonSchemaDefinitionException(
                '{} must be a number'.format(name))
        return n

    def kw_minimum(self, d, checks):
        n = self._number('minimum', d)
        excl = bool(d.get('exclusiveMinimum', False))
        if excl:
            after = ' must be bigger than {}'.format(n)
        else:
            after = ' must be bigger than or equal to {}'.format(n)
        checks.append(dict({'k': 'minimum', 'n': n, 'excl04': excl},
                           **self.err('', after, 'minimum')))

    def kw_maximum(self, d, checks):
        n = self._number('maximum', d)
        excl = bool(d.get('exclusiveMaximum', False))
        if excl:
            after = ' must be smaller than {}'.format(n)
        else:
            after = ' must be smaller than or equal to {}'.format(n)
        checks.append(dict({'k': 'maximum', 'n': n, 'excl04': excl},
                           **self.err('', after, 'maximum')))

    def kw_multipleOf(self, d, checks):
        n = self._number('multipleOf', d)
        checks.append(dict({'k': 'multipleof', 'n': n},
                           **self.err('', ' must be multiple of {}'.format(n),
                                      'multipleOf')))

    def _count(self, name, d):
        n = d[name]
        if not isinstance(n, (int, float)):
            raise JsonSchemaDefinitionException(
                '{} must be a number'.format(name))
        return n

    def kw_minItems(self, d, checks):
        n = self._count('minItems', d)
        checks.append(dict({'k': 'minitems', 'n': n},
                           **self.err('', ' must contain at least {} items'.format(n),
                                      'minItems')))

    def kw_maxItems(self, d, checks):
        n = self._count('maxItems', d)
        checks.append(dict({'k': 'maxitems', 'n': n},
                           **self.err('', ' must contain less than or equal to {} items'.format(n),
                                      'maxItems')))

    def kw_uniqueItems(self, d, checks):
        if not d['uniqueItems']:
            return
        checks.append(dict({'k': 'uniqueitems'},
                           **self.err('', ' must contain unique items', 'uniqueItems')))

    def kw_items(self, d, checks):
        v = self.current_varname
        items = d['items']
        if items is True:
            return
        if items is False:
            checks.append(dict({'k': 'items_false'},
                               **self.err('', ' must not be there', 'items')))
            return
        if isinstance(items, list):
            branches = []
            defaults = []
            for i, item in enumerate(items):
                branches.append(self.sub_level(item, '{}__{}'.format(v, i)))
                if self.use_default and isinstance(item, dict) and 'default' in item:
                    defaults.append(item['default'])
                else:
                    defaults.append(None)
            node = {'k': 'items_tuple', 'branches': branches,
                    'defaults': defaults}
            if 'additionalItems' in d:
                addl = d['additionalItems']
                if addl is False:
                    node['additional'] = False
                else:
                    node['additional'] = self.sub_level(
                        addl, '{}_item'.format(v))
            else:
                node['additional'] = None
            checks.append(dict(node, **self.err('', ' must contain only specified items', 'items')))
            return
        checks.append({'k': 'items_each',
                       'branch': self.sub_level(items, '{}_item'.format(v))})

    def kw_minProperties(self, d, checks):
        n = self._count('minProperties', d)
        checks.append(dict({'k': 'minproperties', 'n': n},
                           **self.err('', ' must contain at least {} properties'.format(n),
                                      'minProperties')))

    def kw_maxProperties(self, d, checks):
        n = self._count('maxProperties', d)
        checks.append(dict({'k': 'maxproperties', 'n': n},
                           **self.err('', ' must contain less than or equal to {} properties'.format(n),
                                      'maxProperties')))

    def kw_required(self, d, checks):
        req = d['required']
        if not isinstance(req, (list, tuple)):
            raise JsonSchemaDefinitionException('required must be an array')
        if len(req) != len(set(req)):
            raise JsonSchemaDefinitionException(
                'required must contain unique elements')
        if not d.get('additionalProperties', True):
            possible = [p for p in req
                        if p not in d.get('properties', {})
                        and not any(re.search(rx, p) for rx in d.get('patternProperties', {}))]
            if possible:
                raise JsonSchemaDefinitionException(
                    '{}: items {} are required but not allowed'.format(
                        self.current_varname, possible))
        checks.append(dict({'k': 'required', 'keys': list(req)},
                           **self.err('', ' must contain ', 'required', dyn='missing')))

    def kw_dependencies(self, d, checks):
        deps = d['dependencies']
        v = self.current_varname
        entries = []
        empty = True
        for key, values in deps.items():
            if values == [] or values is True:
                continue
            empty = False
            skey = self.skey(key)
            if values is False:
                entries.append(('fail', skey, self.err(skey + ' in ', ' must not be there',
                                         'dependencies')))
            elif isinstance(values, list):
                for value in values:
                    entries.append(('missing', skey, (value, self.err(
                        '', ' missing dependency {} for {}'.format(value, skey),
                        'dependencies'))))
            else:
                entries.append(('schema', skey, self.sub_level(values, v)))
        checks.append({'k': 'dependencies', 'entries': entries,
                       'empty': empty})

    def kw_properties(self, d, checks):
        v = self.current_varname
        props = []
        for key, prop in d['properties'].items():
            if not isinstance(prop, (dict, bool)):
                raise JsonSchemaDefinitionException(
                    '{}[{}] must be object'.format(v, self.child_var(key)))
            skey = self.skey(key)
            has_default = (self.use_default and isinstance(prop, dict)
                           and 'default' in prop)
            props.append({'key': skey,
                          'branch': self.sub_level(
                              prop, '{}__{}'.format(v, self.child_var(key))),
                          'default': prop['default'] if has_default else None,
                          'has_default': has_default})
        checks.append({'k': 'properties', 'props': props})

    def kw_patternProperties(self, d, checks):
        if d['patternProperties'] == {}:
            return
        v = self.current_varname
        entries = []
        for pattern, sub in d['patternProperties'].items():
            idx = self.rx(pattern)
            entries.append({'rx': idx,
                            'branch': self.sub_level(sub, '{}_val'.format(v))})
        checks.append({'k': 'patternproperties', 'entries': entries})

    def kw_additionalProperties(self, d, checks):
        v = self.current_varname
        addl = d['additionalProperties']
        props_keys = list(d.get('properties', {}).keys())
        if addl is True or addl == {}:
            checks.append({'k': 'additional_true'})
            return
        if addl:
            checks.append({'k': 'additional_schema',
                           'branch': self.sub_level(addl, '{}_value'.format(v)),
                           'props_keys': props_keys})
        else:
            checks.append(dict(
                {'k': 'additional_false', 'props_keys': props_keys},
                **self.err('', ' must not contain ', 'additionalProperties',
                           dyn='extraset')))

    def kw_exclusiveMinimum(self, d, checks):
        if self.draft == '04':
            return  # draft-04 uses the bool modifier on minimum instead
        import decimal
        n = d['exclusiveMinimum']
        if not isinstance(n, (int, float, decimal.Decimal)):
            raise JsonSchemaDefinitionException(
                'exclusiveMinimum must be an integer, a float or a decimal')
        checks.append(dict({'k': 'exclusiveminimum', 'n': n},
                           **self.err('', ' must be bigger than {}'.format(n),
                                      'exclusiveMinimum')))

    def kw_exclusiveMaximum(self, d, checks):
        if self.draft == '04':
            return  # draft-04 uses the bool modifier on maximum instead
        import decimal
        n = d['exclusiveMaximum']
        if not isinstance(n, (int, float, decimal.Decimal)):
            raise JsonSchemaDefinitionException(
                'exclusiveMaximum must be an integer, a float or a decimal')
        checks.append(dict({'k': 'exclusivemaximum', 'n': n},
                           **self.err('', ' must be smaller than {}'.format(n),
                                      'exclusiveMaximum')))

    def kw_propertyNames(self, d, checks):
        pn = d.get('propertyNames', {})
        v = self.current_varname
        if pn is True:
            return
        if pn is False:
            checks.append(dict(
                {'k': 'propertynames_false'},
                **self.err('', ' must not be there', 'propertyNames')))
            return
        with self.trial_scope():
            sub = self.sub_level(pn, '{}_key'.format(v))
        checks.append(dict({'k': 'propertynames', 'branch': sub},
                           **self.err('', ' must be named by propertyName definition',
                                      'propertyNames')))

    def kw_contains(self, d, checks):
        cd = d['contains']
        v = self.current_varname
        if cd is False:
            checks.append(dict({'k': 'contains_false'},
                               **self.err('', ' is always invalid', 'contains')))
        elif cd is True:
            checks.append(dict({'k': 'contains_true'},
                               **self.err('', ' must not be empty', 'contains')))
        else:
            with self.trial_scope():
                sub = self.sub_level(cd, '{}_key'.format(v))
            checks.append(dict({'k': 'contains', 'branch': sub},
                               **self.err('', ' must contain one of contains definition',
                                          'contains')))

    def kw_const(self, d, checks):
        # Upstream parity: detailed=False + const crashes at compile
        # with KeyError('definition_rule') (unfilled format placeholder).
        if not self.gen._detailed_exceptions:
            raise KeyError('definition_rule')
        const = self.expand(d).get('const')
        # Runtime message: source-embedded e() is parsed back, so a str
        # const renders bare and anything else renders as str().
        shown = const if isinstance(const, str) else str(const)
        after = ' must be same as const definition: {}'.format(shown)
        checks.append(dict({'k': 'const', 'value': d['const']},
                           **self.err('', after, 'const')))

    def kw_if(self, d, checks):
        v = self.current_varname
        with self.trial_scope():
            cond = self.sub_level(d['if'], v)
        then = self.sub_level(d['then'], v) if 'then' in d else None
        els = self.sub_level(d['else'], v) if 'else' in d else None
        checks.append({'k': 'ifthenelse', 'cond': cond,
                       'then': then, 'else': els})

    def kw_contentEncoding(self, d, checks):
        if d['contentEncoding'] == 'base64':
            checks.append(dict(
                {'k': 'contentencoding_base64'},
                **self.err('', ' must be encoded by base64', None)))

    def kw_contentMediaType(self, d, checks):
        if d['contentMediaType'] == 'application/json':
            checks.append(dict(
                {'k': 'contentmediatype_json_bytes'},
                **self.err('', ' must encoded by utf8', None)))
            checks.append(dict(
                {'k': 'contentmediatype_json_str'},
                **self.err('', ' must be valid JSON', None)))

    # -- trial scope ---------------------------------------------
    from contextlib import contextmanager

    @contextmanager
    def trial_scope(self):
        old, self.trial = self.trial, True
        try:
            yield
        finally:
            self.trial = old


def build_plan(definition, handlers, formats, use_default=True,
               use_formats=True, detailed=True, fast_fail=True):
    b = Builder(definition, handlers, formats, use_default, use_formats,
                detailed, fast_fail)
    plan = b.build()
    # Post-pass: resolve ref target uris -> level indices, anywhere nested.
    def resolve(node):
        if isinstance(node, dict):
            if node.get('k') == 'ref' and 'target_uri' in node:
                uri = node.pop('target_uri')
                try:
                    node['target'] = b.memo[uri]
                except KeyError:
                    raise Unsupported('unresolved ref ' + uri)
            for v in node.values():
                resolve(v)
        elif isinstance(node, (list, tuple)):
            for v in node:
                resolve(v)
    resolve(plan['levels'])
    return plan, b
