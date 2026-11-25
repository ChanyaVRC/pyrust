"""Abstract Syntax Trees for Python 3.12."""

from collections import deque
import io
import tokenize

PyCF_ONLY_AST = 1024
PyCF_TYPE_COMMENTS = 4096
PyCF_ALLOW_TOP_LEVEL_AWAIT = 8192


class AST:
    _fields = ()
    _attributes = ()
    __match_args__ = ()

    def __init__(self, *args, **kwargs):
        fields = self._fields
        if len(args) > len(fields):
            n = len(fields)
            plural = '' if n == 1 else 's'
            raise TypeError(
                f'{type(self).__name__} constructor takes at most {n} '
                f'positional argument{plural}'
            )
        for name, val in zip(fields, args):
            setattr(self, name, val)
        for name, val in kwargs.items():
            setattr(self, name, val)


_LOC_ATTRS = ('lineno', 'col_offset', 'end_lineno', 'end_col_offset')
_LOC_DEFAULTS = {'end_lineno': None, 'end_col_offset': None}


def _make_ast_class(name, base, fields=(), attrs=(), defaults=None):
    ns = {
        '__module__': 'ast',
        '_fields': tuple(fields),
        '_attributes': tuple(attrs),
        '__match_args__': tuple(fields),
    }
    if defaults:
        ns.update(defaults)
    cls = type(name, (base,), ns)
    globals()[name] = cls
    return cls


# Abstract base categories
_make_ast_class('mod', AST)
_make_ast_class('stmt', AST, (), _LOC_ATTRS, _LOC_DEFAULTS)
_make_ast_class('expr', AST, (), _LOC_ATTRS, _LOC_DEFAULTS)
_make_ast_class('expr_context', AST)
_make_ast_class('boolop', AST)
_make_ast_class('operator', AST)
_make_ast_class('unaryop', AST)
_make_ast_class('cmpop', AST)
_make_ast_class('comprehension', AST, ('target', 'iter', 'ifs', 'is_async'))
_make_ast_class('excepthandler', AST, (), _LOC_ATTRS, _LOC_DEFAULTS)
_make_ast_class(
    'arguments',
    AST,
    ('posonlyargs', 'args', 'vararg', 'kwonlyargs', 'kw_defaults', 'kwarg', 'defaults'),
    (),
    {'vararg': None, 'kwarg': None},
)
_make_ast_class(
    'arg',
    AST,
    ('arg', 'annotation', 'type_comment'),
    _LOC_ATTRS,
    {'annotation': None, 'type_comment': None, **_LOC_DEFAULTS},
)
_make_ast_class(
    'keyword',
    AST,
    ('arg', 'value'),
    _LOC_ATTRS,
    {'arg': None, **_LOC_DEFAULTS},
)
_make_ast_class(
    'alias',
    AST,
    ('name', 'asname'),
    _LOC_ATTRS,
    {'asname': None, **_LOC_DEFAULTS},
)
_make_ast_class('withitem', AST, ('context_expr', 'optional_vars'), (), {'optional_vars': None})
_make_ast_class('match_case', AST, ('pattern', 'guard', 'body'), (), {'guard': None})
_make_ast_class('pattern', AST, (), _LOC_ATTRS)
_make_ast_class('type_ignore', AST)
_make_ast_class('type_param', AST, (), _LOC_ATTRS)
_make_ast_class('slice', AST)

# Module roots
_make_ast_class('Module', mod, ('body', 'type_ignores'))
_make_ast_class('Interactive', mod, ('body',))
_make_ast_class('Expression', mod, ('body',))
_make_ast_class('FunctionType', mod, ('argtypes', 'returns'))
_make_ast_class('Suite', mod, ())

# Statement nodes
_STMT_SPECS = [
    (
        'FunctionDef',
        ('name', 'args', 'body', 'decorator_list', 'returns', 'type_comment', 'type_params'),
        {'returns': None, 'type_comment': None},
    ),
    (
        'AsyncFunctionDef',
        ('name', 'args', 'body', 'decorator_list', 'returns', 'type_comment', 'type_params'),
        {'returns': None, 'type_comment': None},
    ),
    ('ClassDef', ('name', 'bases', 'keywords', 'body', 'decorator_list', 'type_params'), {}),
    ('Return', ('value',), {'value': None}),
    ('Delete', ('targets',), {}),
    ('Assign', ('targets', 'value', 'type_comment'), {'type_comment': None}),
    ('TypeAlias', ('name', 'type_params', 'value'), {}),
    ('AugAssign', ('target', 'op', 'value'), {}),
    ('AnnAssign', ('target', 'annotation', 'value', 'simple'), {'value': None}),
    ('For', ('target', 'iter', 'body', 'orelse', 'type_comment'), {'type_comment': None}),
    ('AsyncFor', ('target', 'iter', 'body', 'orelse', 'type_comment'), {'type_comment': None}),
    ('While', ('test', 'body', 'orelse'), {}),
    ('If', ('test', 'body', 'orelse'), {}),
    ('With', ('items', 'body', 'type_comment'), {'type_comment': None}),
    ('AsyncWith', ('items', 'body', 'type_comment'), {'type_comment': None}),
    ('Match', ('subject', 'cases'), {}),
    ('Raise', ('exc', 'cause'), {'exc': None, 'cause': None}),
    ('Try', ('body', 'handlers', 'orelse', 'finalbody'), {}),
    ('TryStar', ('body', 'handlers', 'orelse', 'finalbody'), {}),
    ('Assert', ('test', 'msg'), {'msg': None}),
    ('Import', ('names',), {}),
    ('ImportFrom', ('module', 'names', 'level'), {'module': None, 'level': None}),
    ('Global', ('names',), {}),
    ('Nonlocal', ('names',), {}),
    ('Expr', ('value',), {}),
    ('Pass', (), {}),
    ('Break', (), {}),
    ('Continue', (), {}),
]
for _name, _fields, _defs in _STMT_SPECS:
    _make_ast_class(_name, stmt, _fields, _LOC_ATTRS, {**_defs, **_LOC_DEFAULTS})

# Expression nodes
_EXPR_SPECS = [
    ('BoolOp', ('op', 'values'), {}),
    ('NamedExpr', ('target', 'value'), {}),
    ('BinOp', ('left', 'op', 'right'), {}),
    ('UnaryOp', ('op', 'operand'), {}),
    ('Lambda', ('args', 'body'), {}),
    ('IfExp', ('test', 'body', 'orelse'), {}),
    ('Dict', ('keys', 'values'), {}),
    ('Set', ('elts',), {}),
    ('ListComp', ('elt', 'generators'), {}),
    ('SetComp', ('elt', 'generators'), {}),
    ('DictComp', ('key', 'value', 'generators'), {}),
    ('GeneratorExp', ('elt', 'generators'), {}),
    ('Await', ('value',), {}),
    ('Yield', ('value',), {'value': None}),
    ('YieldFrom', ('value',), {}),
    ('Compare', ('left', 'ops', 'comparators'), {}),
    ('Call', ('func', 'args', 'keywords'), {}),
    ('FormattedValue', ('value', 'conversion', 'format_spec'), {'format_spec': None}),
    ('JoinedStr', ('values',), {}),
    ('Constant', ('value', 'kind'), {'kind': None}),
    ('Attribute', ('value', 'attr', 'ctx'), {}),
    ('Subscript', ('value', 'slice', 'ctx'), {}),
    ('Starred', ('value', 'ctx'), {}),
    ('Name', ('id', 'ctx'), {}),
    ('List', ('elts', 'ctx'), {}),
    ('Tuple', ('elts', 'ctx'), {}),
    ('Slice', ('lower', 'upper', 'step'), {'lower': None, 'upper': None, 'step': None}),
]
for _name, _fields, _defs in _EXPR_SPECS:
    _make_ast_class(_name, expr, _fields, _LOC_ATTRS, {**_defs, **_LOC_DEFAULTS})

# Contexts, operators, comparisons, boolops
for _name in ('Load', 'Store', 'Del', 'AugLoad', 'AugStore', 'Param'):
    _make_ast_class(_name, expr_context)

for _name in ('And', 'Or'):
    _make_ast_class(_name, boolop)

for _name in (
    'Add', 'Sub', 'Mult', 'MatMult', 'Div', 'Mod', 'Pow',
    'LShift', 'RShift', 'BitOr', 'BitXor', 'BitAnd', 'FloorDiv',
):
    _make_ast_class(_name, operator)

for _name in ('Invert', 'Not', 'UAdd', 'USub'):
    _make_ast_class(_name, unaryop)

for _name in ('Eq', 'NotEq', 'Lt', 'LtE', 'Gt', 'GtE', 'Is', 'IsNot', 'In', 'NotIn'):
    _make_ast_class(_name, cmpop)

# Exception handlers & type ignores
_make_ast_class(
    'ExceptHandler',
    excepthandler,
    ('type', 'name', 'body'),
    _LOC_ATTRS,
    {'type': None, 'name': None, **_LOC_DEFAULTS},
)
_make_ast_class('TypeIgnore', type_ignore, ('lineno', 'tag'))

# Match patterns (PEP 634)
_make_ast_class('MatchValue', pattern, ('value',), _LOC_ATTRS)
_make_ast_class('MatchSingleton', pattern, ('value',), _LOC_ATTRS)
_make_ast_class('MatchSequence', pattern, ('patterns',), _LOC_ATTRS)
_make_ast_class('MatchMapping', pattern, ('keys', 'patterns', 'rest'), _LOC_ATTRS, {'rest': None})
_make_ast_class('MatchClass', pattern, ('cls', 'patterns', 'kwd_attrs', 'kwd_patterns'), _LOC_ATTRS)
_make_ast_class('MatchStar', pattern, ('name',), _LOC_ATTRS, {'name': None})
_make_ast_class('MatchAs', pattern, ('pattern', 'name'), _LOC_ATTRS, {'pattern': None, 'name': None})
_make_ast_class('MatchOr', pattern, ('patterns',), _LOC_ATTRS)

# Type parameters (PEP 695)
_make_ast_class('TypeVar', type_param, ('name', 'bound'), _LOC_ATTRS, {'bound': None})
_make_ast_class('ParamSpec', type_param, ('name',), _LOC_ATTRS)
_make_ast_class('TypeVarTuple', type_param, ('name',), _LOC_ATTRS)

# Legacy compatibility classes
_make_ast_class('Index', slice)
_make_ast_class('ExtSlice', slice)


class _LegacyConstant(Constant):
    def __new__(cls, *args, **kwargs):
        if cls is Ellipsis:
            kwargs.setdefault('value', ...)
        elif cls is Num and 'n' in kwargs:
            kwargs['value'] = kwargs.pop('n')
        elif cls in (Str, Bytes) and 's' in kwargs:
            kwargs['value'] = kwargs.pop('s')
        inst = Constant.__new__(Constant)
        Constant.__init__(inst, *args, **kwargs)
        return inst


class Num(_LegacyConstant):
    pass


class Str(_LegacyConstant):
    pass


class Bytes(_LegacyConstant):
    pass


class NameConstant(_LegacyConstant):
    pass


class Ellipsis(_LegacyConstant):
    pass


# Class lookup table passed to the native parser.
_AST_CLASSES = {
    name: obj
    for name, obj in list(globals().items())
    if isinstance(obj, type) and issubclass(obj, AST)
}


def _decode_source_bytes(raw):
    data = bytes(raw)
    encoding, _ = tokenize.detect_encoding(io.BytesIO(data).readline)
    if encoding == 'utf-8-sig' and data.startswith(b'\xef\xbb\xbf'):
        data = data[3:]
        encoding = 'utf-8'
    return data.decode(encoding)


def parse(source, filename='<unknown>', mode='exec', *, type_comments=False, feature_version=None):
    """Parse Python source into an AST node."""
    if isinstance(source, AST):
        return source
    if isinstance(source, (bytes, bytearray)):
        source = _decode_source_bytes(source)
    elif not isinstance(source, str):
        raise TypeError(f'parse() arg 1 must be a string, bytes or AST object, not {type(source).__name__}')

    if not isinstance(mode, str):
        raise TypeError(f'parse() mode must be a string, not {type(mode).__name__}')
    if mode not in ('exec', 'eval', 'single', 'func_type'):
        raise ValueError("compile() mode must be 'exec', 'eval' or 'single'")

    if feature_version is None:
        minor = 12
    elif isinstance(feature_version, int):
        minor = feature_version
    elif isinstance(feature_version, tuple) and len(feature_version) == 2:
        major, minor = feature_version
        if major != 3:
            raise ValueError(f'Unsupported major version: {major}')
    else:
        raise TypeError('feature_version must be a 2-tuple of (major, minor) or an int')

    res = _parse_to_ast(source, mode, bool(type_comments), minor, _AST_CLASSES)
    if res[0]:
        return res[1]

    err_kind, msg, lineno, offset, text, end_lineno, end_offset = res[1:]
    if err_kind == 'ValueError':
        raise ValueError(msg)

    exc_cls = SyntaxError
    if err_kind == 'IndentationError':
        exc_cls = IndentationError
    elif err_kind == 'TabError':
        exc_cls = TabError

    exc = exc_cls(msg, (filename, lineno, offset, text))
    exc.end_lineno = end_lineno
    exc.end_offset = end_offset
    raise exc


def literal_eval(node_or_string):
    """Safely evaluate a Python literal expression."""
    if isinstance(node_or_string, str):
        node_or_string = parse(node_or_string.lstrip(' \t'), mode='eval')
    if isinstance(node_or_string, Expression):
        node_or_string = node_or_string.body

    def _malformed(node):
        raise ValueError(f'malformed node or string on line {getattr(node, "lineno", 0)}: {node!r}')

    def _is_plain_number(v):
        return isinstance(v, (int, float, complex)) and not isinstance(v, bool)

    def _is_real_number(v):
        return isinstance(v, (int, float)) and not isinstance(v, bool)

    def _eval_signed(node):
        if isinstance(node, UnaryOp) and isinstance(node.op, (UAdd, USub)):
            operand = node.operand
            if isinstance(operand, Constant) and _is_plain_number(operand.value):
                return +operand.value if isinstance(node.op, UAdd) else -operand.value
            _malformed(node)
        if isinstance(node, Constant):
            return node.value
        _malformed(node)

    def _convert(node):
        if isinstance(node, Constant):
            return node.value
        if isinstance(node, Tuple):
            return tuple(_convert(elt) for elt in node.elts)
        if isinstance(node, List):
            return [_convert(elt) for elt in node.elts]
        if isinstance(node, Set):
            return {_convert(elt) for elt in node.elts}
        if isinstance(node, Call):
            if (
                isinstance(node.func, Name)
                and node.func.id == 'set'
                and not node.args
                and not node.keywords
            ):
                return set()
            _malformed(node)
        if isinstance(node, Dict):
            if len(node.keys) != len(node.values):
                _malformed(node)
            result = {}
            for k, v in zip(node.keys, node.values):
                if k is None:
                    _malformed(node)
                result[_convert(k)] = _convert(v)
            return result
        if isinstance(node, UnaryOp) and isinstance(node.op, (UAdd, USub)):
            return _eval_signed(node)
        if isinstance(node, BinOp) and isinstance(node.op, (Add, Sub)):
            left = _eval_signed(node.left)
            right = node.right
            if (
                _is_real_number(left)
                and isinstance(right, Constant)
                and isinstance(right.value, complex)
            ):
                return left + right.value if isinstance(node.op, Add) else left - right.value
        _malformed(node)

    return _convert(node_or_string)


def iter_fields(node):
    """Yield (fieldname, value) for each field in node._fields present on node."""
    for field in node._fields:
        try:
            yield field, getattr(node, field)
        except AttributeError:
            pass


def iter_child_nodes(node):
    """Yield all direct child AST nodes of node."""
    for _, value in iter_fields(node):
        if isinstance(value, AST):
            yield value
        elif isinstance(value, list):
            for item in value:
                if isinstance(item, AST):
                    yield item


def walk(node):
    """Yield all descendant AST nodes (including node) in breadth-first order."""
    pending = deque([node])
    while pending:
        current = pending.popleft()
        pending.extend(iter_child_nodes(current))
        yield current


def copy_location(new_node, old_node):
    """Copy source location attributes from old_node to new_node."""
    for attr in _LOC_ATTRS:
        if attr in old_node._attributes and attr in new_node._attributes:
            value = getattr(old_node, attr, None)
            if value is not None or (
                hasattr(old_node, attr) and attr.startswith('end_')
            ):
                setattr(new_node, attr, value)
    return new_node


def fix_missing_locations(node):
    """Fill missing lineno/col_offset/end_lineno/end_col_offset attributes."""
    def _fix(n, lineno, col_offset, end_lineno, end_col_offset):
        if 'lineno' in n._attributes:
            if not hasattr(n, 'lineno'):
                n.lineno = lineno
            else:
                lineno = n.lineno
        if 'end_lineno' in n._attributes:
            if getattr(n, 'end_lineno', None) is None:
                n.end_lineno = end_lineno
            else:
                end_lineno = n.end_lineno
        if 'col_offset' in n._attributes:
            if not hasattr(n, 'col_offset'):
                n.col_offset = col_offset
            else:
                col_offset = n.col_offset
        if 'end_col_offset' in n._attributes:
            if getattr(n, 'end_col_offset', None) is None:
                n.end_col_offset = end_col_offset
            else:
                end_col_offset = n.end_col_offset
        for child in iter_child_nodes(n):
            _fix(child, lineno, col_offset, end_lineno, end_col_offset)

    _fix(node, 1, 0, 1, 0)
    return node


def increment_lineno(node, n=1):
    """Increment lineno and end_lineno on node and all its descendants by n."""
    for child in walk(node):
        if isinstance(child, TypeIgnore):
            child.lineno = getattr(child, 'lineno', 0) + n
            continue
        if 'lineno' in child._attributes:
            child.lineno = getattr(child, 'lineno', 0) + n
        if 'end_lineno' in child._attributes:
            end_ln = getattr(child, 'end_lineno', None)
            if end_ln is not None:
                child.end_lineno = end_ln + n
    return node


def dump(node, annotate_fields=True, include_attributes=False, *, indent=None):
    """Return a formatted string representation of an AST node."""
    if not isinstance(node, AST):
        raise TypeError(f'expected AST, got {type(node).__name__!r}')

    if indent is not None and not isinstance(indent, str):
        indent = ' ' * indent

    def _format(item, level=0):
        if indent is not None:
            next_level = level + 1
            prefix = '\n' + indent * next_level
            sep = ',\n' + indent * next_level
        else:
            next_level = level
            prefix = ''
            sep = ', '

        if isinstance(item, AST):
            cls = type(item)
            parts = []
            all_simple = True
            kw_only = annotate_fields
            for field in item._fields:
                try:
                    val = getattr(item, field)
                except AttributeError:
                    kw_only = True
                    continue
                if val is None and getattr(cls, field, ...) is None:
                    kw_only = True
                    continue
                rendered, is_simple = _format(val, next_level)
                all_simple = all_simple and is_simple
                if kw_only:
                    parts.append(f'{field}={rendered}')
                else:
                    parts.append(rendered)
            if include_attributes and item._attributes:
                for attr in item._attributes:
                    try:
                        val = getattr(item, attr)
                    except AttributeError:
                        continue
                    if val is None and getattr(cls, attr, ...) is None:
                        continue
                    rendered, is_simple = _format(val, next_level)
                    all_simple = all_simple and is_simple
                    parts.append(f'{attr}={rendered}')
            if all_simple and len(parts) <= 3:
                return f'{cls.__name__}({", ".join(parts)})', not parts
            return f'{cls.__name__}({prefix}{sep.join(parts)})', False

        if isinstance(item, list):
            if not item:
                return '[]', True
            rendered_items = [_format(x, next_level)[0] for x in item]
            return f'[{prefix}{sep.join(rendered_items)}]', False

        return repr(item), True

    return _format(node)[0]


def _clean_docstring(doc):
    lines = doc.expandtabs().splitlines()
    if not lines:
        return ''
    margin = None
    for line in lines[1:]:
        stripped = line.lstrip(' ')
        if stripped:
            indent = len(line) - len(stripped)
            if margin is None or indent < margin:
                margin = indent
    lines[0] = lines[0].strip(' ')
    if margin is not None:
        for i in range(1, len(lines)):
            lines[i] = lines[i][margin:].rstrip(' ')
    while lines and not lines[-1]:
        lines.pop()
    while lines and not lines[0]:
        lines.pop(0)
    return '\n'.join(lines)


def get_docstring(node, clean=True):
    """Return the docstring for a Module, ClassDef, FunctionDef, or AsyncFunctionDef."""
    if not isinstance(node, (AsyncFunctionDef, FunctionDef, ClassDef, Module)):
        raise TypeError(f"{type(node).__name__!r} can't have docstrings")
    if not (node.body and isinstance(node.body[0], Expr)):
        return None
    val_node = node.body[0].value
    if isinstance(val_node, Constant) and isinstance(val_node.value, str):
        text = val_node.value
    else:
        return None
    if clean:
        return _clean_docstring(text)
    return text


def _split_source_lines(source):
    lines = []
    start = 0
    n = len(source)
    i = 0
    while i < n:
        ch = source[i]
        if ch == '\n':
            lines.append(source[start:i])
            i += 1
            start = i
        elif ch == '\r':
            lines.append(source[start:i])
            if i + 1 < n and source[i + 1] == '\n':
                i += 2
            else:
                i += 1
            start = i
        else:
            i += 1
    if start <= n:
        lines.append(source[start:])
    return lines


def get_source_segment(source, node, *, padded=False):
    """Return the source code segment that generated the given AST node."""
    try:
        if node.end_lineno is None or node.end_col_offset is None:
            return None
        lineno = node.lineno - 1
        end_lineno = node.end_lineno - 1
        col_offset = node.col_offset
        end_col_offset = node.end_col_offset
    except AttributeError:
        return None

    lines = _split_source_lines(source)
    if lineno < 0 or end_lineno < 0 or lineno >= len(lines) or end_lineno >= len(lines):
        return None

    if end_lineno == lineno:
        raw = lines[lineno].encode('utf-8')
        return raw[col_offset:end_col_offset].decode('utf-8')

    first_raw = lines[lineno].encode('utf-8')
    first = first_raw[col_offset:].decode('utf-8')
    if padded:
        prefix = first_raw[:col_offset].decode('utf-8')
        pad = ''.join(ch if ch in (' ', '\t') else ' ' for ch in prefix)
        first = pad + first

    last = lines[end_lineno].encode('utf-8')[:end_col_offset].decode('utf-8')
    middle = lines[lineno + 1 : end_lineno]
    return '\n'.join([first, *middle, last])


class NodeVisitor:
    """Base visitor that dispatches to visit_<ClassName> methods."""

    def visit(self, node):
        method = 'visit_' + type(node).__name__
        visitor = getattr(self, method, self.generic_visit)
        return visitor(node)

    def generic_visit(self, node):
        for _, value in iter_fields(node):
            if isinstance(value, list):
                for item in value:
                    if isinstance(item, AST):
                        self.visit(item)
            elif isinstance(value, AST):
                self.visit(value)


class NodeTransformer(NodeVisitor):
    """Visitor subclass that allows in-place AST rewriting."""

    def generic_visit(self, node):
        for field, old_value in iter_fields(node):
            if isinstance(old_value, list):
                updated = []
                for item in old_value:
                    if isinstance(item, AST):
                        item = self.visit(item)
                        if item is None:
                            continue
                        if not isinstance(item, AST):
                            updated.extend(item)
                            continue
                    updated.append(item)
                old_value[:] = updated
            elif isinstance(old_value, AST):
                new_node = self.visit(old_value)
                if new_node is None:
                    delattr(node, field)
                else:
                    setattr(node, field, new_node)
        return node


# ── AST Unparser (Python 3.12) ────────────────────────────────────────────

_PREC_TUPLE = 0
_PREC_YIELD = 1
_PREC_NAMED = 2
_PREC_TEST = 3
_PREC_OR = 4
_PREC_AND = 5
_PREC_NOT = 6
_PREC_CMP = 7
_PREC_BITOR = 8
_PREC_BITXOR = 9
_PREC_BITAND = 10
_PREC_SHIFT = 11
_PREC_ARITH = 12
_PREC_TERM = 13
_PREC_FACTOR = 14
_PREC_POWER = 15
_PREC_AWAIT = 16
_PREC_ATOM = 17

_BINOP_SYMBOLS = {
    Add: ('+', _PREC_ARITH),
    Sub: ('-', _PREC_ARITH),
    Mult: ('*', _PREC_TERM),
    MatMult: ('@', _PREC_TERM),
    Div: ('/', _PREC_TERM),
    FloorDiv: ('//', _PREC_TERM),
    Mod: ('%', _PREC_TERM),
    LShift: ('<<', _PREC_SHIFT),
    RShift: ('>>', _PREC_SHIFT),
    BitOr: ('|', _PREC_BITOR),
    BitXor: ('^', _PREC_BITXOR),
    BitAnd: ('&', _PREC_BITAND),
    Pow: ('**', _PREC_POWER),
}

_UNARYOP_SYMBOLS = {
    Invert: ('~', _PREC_FACTOR),
    Not: ('not ', _PREC_NOT),
    UAdd: ('+', _PREC_FACTOR),
    USub: ('-', _PREC_FACTOR),
}

_CMPOP_SYMBOLS = {
    Eq: '==',
    NotEq: '!=',
    Lt: '<',
    LtE: '<=',
    Gt: '>',
    GtE: '>=',
    Is: 'is',
    IsNot: 'is not',
    In: 'in',
    NotIn: 'not in',
}

_BOOLOP_SYMBOLS = {
    And: ('and', _PREC_AND),
    Or: ('or', _PREC_OR),
}

_CONV_CHARS = {115: 's', 114: 'r', 97: 'a'}


class _AstSerializer:
    def __init__(self):
        self._buf = []
        self._indent = 0

    def emit(self, text):
        self._buf.append(text)

    def newline(self, text=''):
        if self._buf:
            self._buf.append('\n')
        self._buf.append('    ' * self._indent)
        if text:
            self._buf.append(text)

    def serialize(self, node):
        self._dispatch(node)
        return ''.join(self._buf)

    def _dispatch(self, node):
        handler = getattr(self, '_node_' + type(node).__name__, None)
        if handler is None:
            raise TypeError(f'Cannot unparse AST node of type {type(node).__name__!r}')
        handler(node)

    def _expr(self, node, prec=_PREC_TUPLE):
        s = self._render_expr(node, prec)
        self.emit(s)

    def _render_block(self, stmts):
        self._indent += 1
        if not stmts:
            self.newline('pass')
        else:
            for st in stmts:
                self._dispatch(st)
        self._indent -= 1

    # ── Roots ──

    def _node_Module(self, node):
        for st in node.body:
            self._dispatch(st)

    def _node_Interactive(self, node):
        for st in node.body:
            self._dispatch(st)

    def _node_Expression(self, node):
        self.emit(self._render_expr(node.body, _PREC_TUPLE))

    def _node_FunctionType(self, node):
        args_str = ', '.join(self._render_expr(a, _PREC_TEST) for a in node.argtypes)
        ret_str = self._render_expr(node.returns, _PREC_TEST)
        self.emit(f'({args_str}) -> {ret_str}')

    # ── Statements ──

    def _node_Expr(self, node):
        self.newline()
        self.emit(self._render_expr(node.value, _PREC_TUPLE))

    def _node_Pass(self, node):
        self.newline('pass')

    def _node_Break(self, node):
        self.newline('break')

    def _node_Continue(self, node):
        self.newline('continue')

    def _node_Return(self, node):
        self.newline('return')
        if node.value is not None:
            self.emit(' ' + self._render_expr(node.value, _PREC_TUPLE))

    def _node_Delete(self, node):
        self.newline('del ')
        self.emit(', '.join(self._render_expr(t, _PREC_TEST) for t in node.targets))

    def _node_Assign(self, node):
        self.newline()
        for t in node.targets:
            self.emit(self._render_expr(t, _PREC_TUPLE) + ' = ')
        self.emit(self._render_expr(node.value, _PREC_TUPLE))
        if getattr(node, 'type_comment', None):
            self.emit(f'  # type: {node.type_comment}')

    def _node_AugAssign(self, node):
        sym, _ = _BINOP_SYMBOLS[type(node.op)]
        self.newline()
        self.emit(
            f'{self._render_expr(node.target, _PREC_TEST)} {sym}= '
            f'{self._render_expr(node.value, _PREC_TUPLE)}'
        )

    def _node_AnnAssign(self, node):
        self.newline()
        target_str = self._render_expr(node.target, _PREC_TEST)
        if not node.simple and isinstance(node.target, Name):
            target_str = f'({target_str})'
        ann_str = self._render_expr(node.annotation, _PREC_TEST)
        if node.value is not None:
            val_str = self._render_expr(node.value, _PREC_TEST)
            self.emit(f'{target_str}: {ann_str} = {val_str}')
        else:
            self.emit(f'{target_str}: {ann_str}')

    def _node_TypeAlias(self, node):
        self.newline('type ')
        self.emit(self._render_expr(node.name, _PREC_ATOM))
        tp = self._render_type_params(getattr(node, 'type_params', []))
        if tp:
            self.emit(tp)
        self.emit(' = ' + self._render_expr(node.value, _PREC_TEST))

    def _node_Global(self, node):
        self.newline('global ' + ', '.join(node.names))

    def _node_Nonlocal(self, node):
        self.newline('nonlocal ' + ', '.join(node.names))

    def _node_Assert(self, node):
        self.newline('assert ' + self._render_expr(node.test, _PREC_TEST))
        if node.msg is not None:
            self.emit(', ' + self._render_expr(node.msg, _PREC_TEST))

    def _node_Raise(self, node):
        self.newline('raise')
        if node.exc is not None:
            self.emit(' ' + self._render_expr(node.exc, _PREC_TEST))
            if node.cause is not None:
                self.emit(' from ' + self._render_expr(node.cause, _PREC_TEST))

    def _format_alias(self, al):
        return f'{al.name} as {al.asname}' if al.asname else al.name

    def _node_Import(self, node):
        self.newline('import ' + ', '.join(self._format_alias(a) for a in node.names))

    def _node_ImportFrom(self, node):
        dots = '.' * (node.level or 0)
        mod_name = node.module or ''
        names = ', '.join(self._format_alias(a) for a in node.names)
        self.newline(f'from {dots}{mod_name} import {names}')

    def _render_type_params(self, params):
        if not params:
            return ''
        rendered = []
        for p in params:
            if isinstance(p, TypeVar):
                if p.bound is not None:
                    rendered.append(f'{p.name}: {self._render_expr(p.bound, _PREC_TEST)}')
                else:
                    rendered.append(p.name)
            elif isinstance(p, TypeVarTuple):
                rendered.append(f'*{p.name}')
            elif isinstance(p, ParamSpec):
                rendered.append(f'**{p.name}')
        return '[' + ', '.join(rendered) + ']'

    def _render_arg(self, a):
        if a.annotation is not None:
            return f'{a.arg}: {self._render_expr(a.annotation, _PREC_TEST)}'
        return a.arg

    def _render_arguments(self, a):
        parts = []
        all_pos = list(a.posonlyargs) + list(a.args)
        defaults = [None] * (len(all_pos) - len(a.defaults)) + list(a.defaults)
        posonly_count = len(a.posonlyargs)
        for idx, (param, default) in enumerate(zip(all_pos, defaults)):
            text = self._render_arg(param)
            if default is not None:
                text += '=' + self._render_expr(default, _PREC_TEST)
            parts.append(text)
            if posonly_count and idx == posonly_count - 1:
                parts.append('/')
        if a.vararg is not None:
            parts.append('*' + self._render_arg(a.vararg))
        elif a.kwonlyargs:
            parts.append('*')
        for param, default in zip(a.kwonlyargs, a.kw_defaults):
            text = self._render_arg(param)
            if default is not None:
                text += '=' + self._render_expr(default, _PREC_TEST)
            parts.append(text)
        if a.kwarg is not None:
            parts.append('**' + self._render_arg(a.kwarg))
        return ', '.join(parts)

    def _emit_funcdef(self, node, prefix):
        for dec in node.decorator_list:
            self.newline('@' + self._render_expr(dec, _PREC_TEST))
        tp = self._render_type_params(getattr(node, 'type_params', []))
        args_str = self._render_arguments(node.args)
        ret = ''
        if node.returns is not None:
            ret = ' -> ' + self._render_expr(node.returns, _PREC_TEST)
        self.newline(f'{prefix}def {node.name}{tp}({args_str}){ret}:')
        if getattr(node, 'type_comment', None):
            self.emit(f'  # type: {node.type_comment}')
        self._render_block(node.body)

    def _node_FunctionDef(self, node):
        self._emit_funcdef(node, '')

    def _node_AsyncFunctionDef(self, node):
        self._emit_funcdef(node, 'async ')

    def _node_ClassDef(self, node):
        for dec in node.decorator_list:
            self.newline('@' + self._render_expr(dec, _PREC_TEST))
        tp = self._render_type_params(getattr(node, 'type_params', []))
        header_args = [self._render_expr(b, _PREC_TEST) for b in node.bases]
        for kw in node.keywords:
            if kw.arg is None:
                header_args.append('**' + self._render_expr(kw.value, _PREC_TEST))
            else:
                header_args.append(f'{kw.arg}={self._render_expr(kw.value, _PREC_TEST)}')
        parens = f'({", ".join(header_args)})' if header_args else ''
        self.newline(f'class {node.name}{tp}{parens}:')
        self._render_block(node.body)

    def _node_If(self, node):
        self.newline(f'if {self._render_expr(node.test, _PREC_TEST)}:')
        self._render_block(node.body)
        orelse = node.orelse
        while len(orelse) == 1 and isinstance(orelse[0], If):
            nxt = orelse[0]
            self.newline(f'elif {self._render_expr(nxt.test, _PREC_TEST)}:')
            self._render_block(nxt.body)
            orelse = nxt.orelse
        if orelse:
            self.newline('else:')
            self._render_block(orelse)

    def _node_While(self, node):
        self.newline(f'while {self._render_expr(node.test, _PREC_TEST)}:')
        self._render_block(node.body)
        if node.orelse:
            self.newline('else:')
            self._render_block(node.orelse)

    def _emit_for(self, node, prefix):
        target = self._render_expr(node.target, _PREC_TUPLE)
        it = self._render_expr(node.iter, _PREC_TUPLE)
        self.newline(f'{prefix}for {target} in {it}:')
        if getattr(node, 'type_comment', None):
            self.emit(f'  # type: {node.type_comment}')
        self._render_block(node.body)
        if node.orelse:
            self.newline('else:')
            self._render_block(node.orelse)

    def _node_For(self, node):
        self._emit_for(node, '')

    def _node_AsyncFor(self, node):
        self._emit_for(node, 'async ')

    def _emit_with(self, node, prefix):
        items = []
        for it in node.items:
            s = self._render_expr(it.context_expr, _PREC_TEST)
            if it.optional_vars is not None:
                s += ' as ' + self._render_expr(it.optional_vars, _PREC_TEST)
            items.append(s)
        self.newline(f'{prefix}with {", ".join(items)}:')
        if getattr(node, 'type_comment', None):
            self.emit(f'  # type: {node.type_comment}')
        self._render_block(node.body)

    def _node_With(self, node):
        self._emit_with(node, '')

    def _node_AsyncWith(self, node):
        self._emit_with(node, 'async ')

    def _emit_try(self, node, star):
        self.newline('try:')
        self._render_block(node.body)
        kw = 'except*' if star else 'except'
        for h in node.handlers:
            header = kw
            if h.type is not None:
                header += ' ' + self._render_expr(h.type, _PREC_TEST)
                if h.name:
                    header += f' as {h.name}'
            self.newline(header + ':')
            self._render_block(h.body)
        if node.orelse:
            self.newline('else:')
            self._render_block(node.orelse)
        if node.finalbody:
            self.newline('finally:')
            self._render_block(node.finalbody)

    def _node_Try(self, node):
        self._emit_try(node, False)

    def _node_TryStar(self, node):
        self._emit_try(node, True)

    def _node_Match(self, node):
        self.newline(f'match {self._render_expr(node.subject, _PREC_TUPLE)}:')
        self._indent += 1
        for case in node.cases:
            pat = self._render_pattern(case.pattern)
            if case.guard is not None:
                self.newline(f'case {pat} if {self._render_expr(case.guard, _PREC_TEST)}:')
            else:
                self.newline(f'case {pat}:')
            self._render_block(case.body)
        self._indent -= 1

    # ── Patterns ──

    def _render_pattern(self, pat, in_or=False):
        if isinstance(pat, MatchValue):
            return self._render_expr(pat.value, _PREC_TEST)
        if isinstance(pat, MatchSingleton):
            return repr(pat.value)
        if isinstance(pat, MatchSequence):
            inner = ', '.join(self._render_pattern(p) for p in pat.patterns)
            return f'[{inner}]'
        if isinstance(pat, MatchStar):
            return '*' + (pat.name if pat.name else '_')
        if isinstance(pat, MatchMapping):
            pairs = [
                f'{self._render_expr(k, _PREC_TEST)}: {self._render_pattern(p)}'
                for k, p in zip(pat.keys, pat.patterns)
            ]
            if pat.rest:
                pairs.append(f'**{pat.rest}')
            return '{' + ', '.join(pairs) + '}'
        if isinstance(pat, MatchClass):
            cls_s = self._render_expr(pat.cls, _PREC_ATOM)
            parts = [self._render_pattern(p) for p in pat.patterns]
            for attr, p in zip(pat.kwd_attrs, pat.kwd_patterns):
                parts.append(f'{attr}={self._render_pattern(p)}')
            return f'{cls_s}({", ".join(parts)})'
        if isinstance(pat, MatchAs):
            if pat.name is None:
                return '_'
            if pat.pattern is None:
                return pat.name
            res = f'{self._render_pattern(pat.pattern, in_or=True)} as {pat.name}'
            return f'({res})' if in_or else res
        if isinstance(pat, MatchOr):
            res = ' | '.join(self._render_pattern(p, in_or=True) for p in pat.patterns)
            return f'({res})' if in_or else res
        raise TypeError(f'Unknown pattern node: {type(pat).__name__}')

    # ── Expressions ──

    def _wrap(self, text, my_prec, parent_prec):
        if my_prec < parent_prec:
            return f'({text})'
        return text

    def _render_constant_value(self, val):
        if val is ...:
            return '...'
        if isinstance(val, tuple):
            if len(val) == 1:
                return f'({self._render_constant_value(val[0])},)'
            return '(' + ', '.join(self._render_constant_value(x) for x in val) + ')'
        if isinstance(val, float):
            if val != val:
                return '1e309 - 1e309'
            if val == float('inf'):
                return '1e309'
            if val == float('-inf'):
                return '-1e309'
        if isinstance(val, complex):
            if val.real == 0.0:
                im = val.imag
                if im == float('inf'):
                    return '1e309j'
                if im == float('-inf'):
                    return '-1e309j'
                return repr(val)
        return repr(val)

    def _render_comprehension(self, gen):
        prefix = 'async for ' if gen.is_async else 'for '
        tgt = self._render_expr(gen.target, _PREC_TUPLE)
        it = self._render_expr(gen.iter, _PREC_OR)
        s = f'{prefix}{tgt} in {it}'
        for cond in gen.ifs:
            s += ' if ' + self._render_expr(cond, _PREC_OR)
        return s

    def _render_fstring_parts(self, values):
        chunks = []
        for v in values:
            if isinstance(v, Constant) and isinstance(v.value, str):
                escaped = v.value.replace('{', '{{').replace('}', '}}')
                chunks.append(escaped)
            elif isinstance(v, FormattedValue):
                inner = self._render_expr(v.value, _PREC_TEST)
                if inner.startswith('{'):
                    inner = ' ' + inner
                conv = ''
                if v.conversion in _CONV_CHARS:
                    conv = '!' + _CONV_CHARS[v.conversion]
                spec = ''
                if v.format_spec is not None:
                    spec = ':' + self._render_fstring_parts(v.format_spec.values)
                chunks.append('{' + inner + conv + spec + '}')
        return ''.join(chunks)

    def _render_joined_str(self, node):
        body = self._render_fstring_parts(node.values)
        # Pick a quote delimiter that does not collide with the rendered body.
        for q in ("'", '"', "'''", '"""'):
            if q not in body and not body.endswith('\\'):
                if q in ("'", '"') and '\n' in body:
                    continue
                return 'f' + q + body + q
        # Fallback: escape newlines and single quotes in literal pieces.
        safe = []
        for v in node.values:
            if isinstance(v, Constant) and isinstance(v.value, str):
                lit = repr(v.value)[1:-1].replace('{', '{{').replace('}', '}}')
                safe.append(lit)
            elif isinstance(v, FormattedValue):
                inner = self._render_expr(v.value, _PREC_TEST)
                if inner.startswith('{'):
                    inner = ' ' + inner
                conv = ('!' + _CONV_CHARS[v.conversion]) if v.conversion in _CONV_CHARS else ''
                spec = ''
                if v.format_spec is not None:
                    spec = ':' + self._render_fstring_parts(v.format_spec.values)
                safe.append('{' + inner + conv + spec + '}')
        joined = ''.join(safe)
        if "'" not in joined:
            return f"f'{joined}'"
        if '"' not in joined:
            return f'f"{joined}"'
        return f'f"""{joined}"""'

    def _render_expr(self, node, prec):
        if isinstance(node, Constant):
            s = self._render_constant_value(node.value)
            if isinstance(node.value, (int, float, complex)) and not isinstance(node.value, bool):
                if s.startswith('-'):
                    return self._wrap(s, _PREC_FACTOR, prec)
            return s

        if isinstance(node, Name):
            return node.id

        if isinstance(node, Attribute):
            val_str = self._render_expr(node.value, _PREC_ATOM)
            if (
                isinstance(node.value, Constant)
                and isinstance(node.value.value, int)
                and not isinstance(node.value.value, bool)
            ):
                val_str += ' '
            return f'{val_str}.{node.attr}'

        if isinstance(node, Subscript):
            val_str = self._render_expr(node.value, _PREC_ATOM)
            if isinstance(node.slice, Tuple) and node.slice.elts:
                if len(node.slice.elts) == 1:
                    sl_str = self._render_expr(node.slice.elts[0], _PREC_TEST) + ','
                else:
                    sl_str = ', '.join(self._render_expr(e, _PREC_TEST) for e in node.slice.elts)
            else:
                sl_str = self._render_expr(node.slice, _PREC_TEST)
            return f'{val_str}[{sl_str}]'

        if isinstance(node, Slice):
            lo = '' if node.lower is None else self._render_expr(node.lower, _PREC_TEST)
            hi = '' if node.upper is None else self._render_expr(node.upper, _PREC_TEST)
            if node.step is None:
                return f'{lo}:{hi}'
            st = self._render_expr(node.step, _PREC_TEST)
            return f'{lo}:{hi}:{st}'

        if isinstance(node, Starred):
            return '*' + self._render_expr(node.value, _PREC_BITOR)

        if isinstance(node, List):
            return '[' + ', '.join(self._render_expr(e, _PREC_TEST) for e in node.elts) + ']'

        if isinstance(node, Tuple):
            if not node.elts:
                return '()'
            if len(node.elts) == 1:
                inner = self._render_expr(node.elts[0], _PREC_TEST) + ','
            else:
                inner = ', '.join(self._render_expr(e, _PREC_TEST) for e in node.elts)
            return self._wrap(inner, _PREC_TUPLE, prec)

        if isinstance(node, Set):
            if not node.elts:
                return 'set()'
            return '{' + ', '.join(self._render_expr(e, _PREC_TEST) for e in node.elts) + '}'

        if isinstance(node, Dict):
            items = []
            for k, v in zip(node.keys, node.values):
                if k is None:
                    items.append('**' + self._render_expr(v, _PREC_BITOR))
                else:
                    items.append(
                        f'{self._render_expr(k, _PREC_TEST)}: {self._render_expr(v, _PREC_TEST)}'
                    )
            return '{' + ', '.join(items) + '}'

        if isinstance(node, ListComp):
            elt = self._render_expr(node.elt, _PREC_NAMED)
            gens = ' '.join(self._render_comprehension(g) for g in node.generators)
            return f'[{elt} {gens}]'

        if isinstance(node, SetComp):
            elt = self._render_expr(node.elt, _PREC_NAMED)
            gens = ' '.join(self._render_comprehension(g) for g in node.generators)
            return f'{{{elt} {gens}}}'

        if isinstance(node, DictComp):
            k = self._render_expr(node.key, _PREC_TEST)
            v = self._render_expr(node.value, _PREC_TEST)
            gens = ' '.join(self._render_comprehension(g) for g in node.generators)
            return f'{{{k}: {v} {gens}}}'

        if isinstance(node, GeneratorExp):
            elt = self._render_expr(node.elt, _PREC_NAMED)
            gens = ' '.join(self._render_comprehension(g) for g in node.generators)
            return f'({elt} {gens})'

        if isinstance(node, Call):
            func_s = self._render_expr(node.func, _PREC_ATOM)
            parts = []
            for idx, arg in enumerate(node.args):
                if (
                    len(node.args) == 1
                    and not node.keywords
                    and isinstance(arg, GeneratorExp)
                ):
                    elt = self._render_expr(arg.elt, _PREC_NAMED)
                    gens = ' '.join(self._render_comprehension(g) for g in arg.generators)
                    parts.append(f'{elt} {gens}')
                else:
                    parts.append(self._render_expr(arg, _PREC_NAMED))
            for kw in node.keywords:
                if kw.arg is None:
                    parts.append('**' + self._render_expr(kw.value, _PREC_TEST))
                else:
                    parts.append(f'{kw.arg}={self._render_expr(kw.value, _PREC_TEST)}')
            return f'{func_s}({", ".join(parts)})'

        if isinstance(node, UnaryOp):
            sym, my_prec = _UNARYOP_SYMBOLS[type(node.op)]
            operand_s = self._render_expr(node.operand, my_prec)
            return self._wrap(f'{sym}{operand_s}', my_prec, prec)

        if isinstance(node, BinOp):
            sym, my_prec = _BINOP_SYMBOLS[type(node.op)]
            if isinstance(node.op, Pow):
                left_s = self._render_expr(node.left, my_prec + 1)
                right_s = self._render_expr(node.right, my_prec - 1)
            else:
                left_s = self._render_expr(node.left, my_prec)
                right_s = self._render_expr(node.right, my_prec + 1)
            return self._wrap(f'{left_s} {sym} {right_s}', my_prec, prec)

        if isinstance(node, BoolOp):
            sym, my_prec = _BOOLOP_SYMBOLS[type(node.op)]
            parts = [self._render_expr(v, my_prec + 1) for v in node.values]
            return self._wrap(f' {sym} '.join(parts), my_prec, prec)

        if isinstance(node, Compare):
            pieces = [self._render_expr(node.left, _PREC_CMP + 1)]
            for op, comp in zip(node.ops, node.comparators):
                pieces.append(_CMPOP_SYMBOLS[type(op)])
                pieces.append(self._render_expr(comp, _PREC_CMP + 1))
            return self._wrap(' '.join(pieces), _PREC_CMP, prec)

        if isinstance(node, IfExp):
            body_s = self._render_expr(node.body, _PREC_OR)
            test_s = self._render_expr(node.test, _PREC_OR)
            else_s = self._render_expr(node.orelse, _PREC_TEST)
            return self._wrap(f'{body_s} if {test_s} else {else_s}', _PREC_TEST, prec)

        if isinstance(node, Lambda):
            args_s = self._render_arguments(node.args)
            body_s = self._render_expr(node.body, _PREC_TEST)
            head = f'lambda {args_s}: {body_s}' if args_s else f'lambda: {body_s}'
            return self._wrap(head, _PREC_TEST, prec)

        if isinstance(node, NamedExpr):
            t_s = self._render_expr(node.target, _PREC_ATOM)
            v_s = self._render_expr(node.value, _PREC_TEST)
            return self._wrap(f'{t_s} := {v_s}', _PREC_NAMED, max(prec, _PREC_TEST))

        if isinstance(node, Await):
            val_s = self._render_expr(node.value, _PREC_ATOM)
            return self._wrap(f'await {val_s}', _PREC_AWAIT, prec)

        if isinstance(node, Yield):
            if node.value is None:
                s = 'yield'
            else:
                s = 'yield ' + self._render_expr(node.value, _PREC_TUPLE)
            return self._wrap(s, _PREC_YIELD, prec)

        if isinstance(node, YieldFrom):
            s = 'yield from ' + self._render_expr(node.value, _PREC_TEST)
            return self._wrap(s, _PREC_YIELD, prec)

        if isinstance(node, JoinedStr):
            return self._render_joined_str(node)

        if isinstance(node, FormattedValue):
            return self._render_joined_str(JoinedStr(values=[node]))

        raise TypeError(f'Cannot unparse expression node {type(node).__name__!r}')


def unparse(ast_obj):
    """Unparse an AST object back into Python 3.12 source code."""
    return _AstSerializer().serialize(ast_obj)


def main():
    import sys
    src = sys.stdin.read()
    tree = parse(src)
    print(dump(tree, indent=3))
