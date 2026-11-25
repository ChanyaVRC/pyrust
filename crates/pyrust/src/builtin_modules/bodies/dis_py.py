"""Disassembler of Python byte code into mnemonics."""

import collections
import io
import sys
import types

from opcode import *
from opcode import (
    __all__ as _opcodes_all,
    _cache_format,
    _inline_cache_entries,
    _nb_ops,
    _intrinsic_1_descs,
    _intrinsic_2_descs,
    _specializations,
    _specialized_instructions,
)

__all__ = [
    "code_info",
    "dis",
    "disassemble",
    "distb",
    "disco",
    "findlinestarts",
    "findlabels",
    "show_code",
    "get_instructions",
    "Instruction",
    "Bytecode",
] + list(_opcodes_all)
del _opcodes_all

_have_code = (
    types.MethodType,
    types.FunctionType,
    classmethod,
    staticmethod,
    type,
)


def _is_have_code(obj):
    return (
        isinstance(obj, _have_code)
        or isinstance(obj, types.CodeType)
        or type(obj) is types.CodeType
    )


FORMAT_VALUE = opmap['FORMAT_VALUE']
FORMAT_VALUE_CONVERTERS = (
    (None, ''),
    (str, 'str'),
    (repr, 'repr'),
    (ascii, 'ascii'),
)
MAKE_FUNCTION = opmap['MAKE_FUNCTION']
MAKE_FUNCTION_FLAGS = ('defaults', 'kwdefaults', 'annotations', 'closure')

LOAD_CONST = opmap['LOAD_CONST']
RETURN_CONST = opmap['RETURN_CONST']
LOAD_GLOBAL = opmap['LOAD_GLOBAL']
BINARY_OP = opmap['BINARY_OP']
JUMP_BACKWARD = opmap['JUMP_BACKWARD']
FOR_ITER = opmap['FOR_ITER']
SEND = opmap['SEND']
LOAD_ATTR = opmap['LOAD_ATTR']
LOAD_SUPER_ATTR = opmap['LOAD_SUPER_ATTR']
CALL_INTRINSIC_1 = opmap['CALL_INTRINSIC_1']
CALL_INTRINSIC_2 = opmap['CALL_INTRINSIC_2']

CACHE = opmap["CACHE"]

_all_opname = list(opname)
_all_opmap = dict(opmap)
_empty_slot = [slot for slot, name in enumerate(_all_opname) if name.startswith("<")]
for _spec_op, _specialized in zip(_empty_slot, _specialized_instructions):
    _all_opname[_spec_op] = _specialized
    _all_opmap[_specialized] = _spec_op
del _spec_op, _specialized

deoptmap = {
    specialized: base
    for base, family in _specializations.items()
    for specialized in family
}


def _try_compile(source, name):
    """Attempts to compile the given source, first as an expression and
    then as a statement if the first approach fails.

    Utility function to accept strings in functions that otherwise
    expect code objects.
    """
    try:
        return compile(source, name, 'eval')
    except SyntaxError:
        pass
    return compile(source, name, 'exec')


def dis(x=None, *, file=None, depth=None, show_caches=False, adaptive=False):
    """Disassemble classes, methods, functions, and other compiled objects.

    With no argument, disassemble the last traceback.

    Compiled objects currently include generator objects, async generator
    objects, and coroutine objects, all of which store their code object
    in a special attribute.
    """
    if x is None:
        distb(file=file, show_caches=show_caches, adaptive=adaptive)
        return
    # Extract functions from methods / descriptors.
    if hasattr(x, '__func__'):
        x = x.__func__
    # Extract compiled code objects from...
    if hasattr(x, '__code__'):  # ...a function, or
        x = x.__code__
    elif hasattr(x, 'gi_code'):  # ...a generator object, or
        x = x.gi_code
    elif hasattr(x, 'ag_code'):  # ...an asynchronous generator object, or
        x = x.ag_code
    elif hasattr(x, 'cr_code'):  # ...a coroutine.
        x = x.cr_code
    # Perform the disassembly.
    if hasattr(x, '__dict__'):  # Class or module
        items = sorted(x.__dict__.items())
        for name, x1 in items:
            if _is_have_code(x1):
                print("Disassembly of %s:" % name, file=file)
                try:
                    dis(x1, file=file, depth=depth, show_caches=show_caches, adaptive=adaptive)
                except TypeError as msg:
                    print("Sorry:", msg, file=file)
                print(file=file)
    elif hasattr(x, 'co_code'):  # Code object
        _disassemble_recursive(
            x, file=file, depth=depth, show_caches=show_caches, adaptive=adaptive
        )
    elif isinstance(x, (bytes, bytearray)):  # Raw bytecode
        _disassemble_bytes(x, file=file, show_caches=show_caches)
    elif isinstance(x, str):  # Source code
        _disassemble_str(
            x, file=file, depth=depth, show_caches=show_caches, adaptive=adaptive
        )
    else:
        raise TypeError(
            "don't know how to disassemble %s objects" % type(x).__name__
        )


def _tb_code_object(tb):
    """Resolve the code object for a traceback node, upgrading a reconstructed
    frame's lightweight code object to the live function's `__code__` when
    needed so `distb()` and `Bytecode.from_traceback()` can inspect `FnCode`."""
    frame = tb.tb_frame
    co = frame.f_code
    if not co.co_code and _dis_fncode_info(co) is None:
        name = co.co_name
        if name and name != '<module>':
            candidate = frame.f_globals.get(name)
            cand_code = getattr(candidate, '__code__', None)
            if cand_code is not None and _dis_fncode_info(cand_code) is not None:
                if cand_code.co_filename == co.co_filename:
                    return cand_code
    return co


def distb(tb=None, *, file=None, show_caches=False, adaptive=False):
    """Disassemble a traceback (default: last traceback)."""
    if tb is None:
        try:
            if hasattr(sys, 'last_exc'):
                tb = sys.last_exc.__traceback__
            else:
                tb = sys.last_traceback
        except AttributeError:
            raise RuntimeError("no last traceback to disassemble") from None
        while tb.tb_next:
            tb = tb.tb_next
    disassemble(
        _tb_code_object(tb),
        tb.tb_lasti,
        file=file,
        show_caches=show_caches,
        adaptive=adaptive,
    )


# The inspect module interrogates this dictionary to build its
# list of CO_* constants. It is also used by pretty_flags to
# turn the co_flags field into a human readable list.
COMPILER_FLAG_NAMES = {
    1: "OPTIMIZED",
    2: "NEWLOCALS",
    4: "VARARGS",
    8: "VARKEYWORDS",
    16: "NESTED",
    32: "GENERATOR",
    64: "NOFREE",
    128: "COROUTINE",
    256: "ITERABLE_COROUTINE",
    512: "ASYNC_GENERATOR",
}


def pretty_flags(flags):
    """Return pretty representation of code flags."""
    names = []
    for i in range(32):
        flag = 1 << i
        if flags & flag:
            names.append(COMPILER_FLAG_NAMES.get(flag, hex(flag)))
            flags ^= flag
            if not flags:
                break
    else:
        names.append(hex(flags))
    return ", ".join(names)


class _Unknown:
    def __repr__(self):
        return "<unknown>"


# Sentinel to represent values that cannot be calculated
UNKNOWN = _Unknown()


def _get_code_object(x):
    """Helper to handle methods, compiled or raw code objects, and strings."""
    # Extract functions from methods.
    if hasattr(x, '__func__'):
        x = x.__func__
    # Extract compiled code objects from...
    if hasattr(x, '__code__'):  # ...a function, or
        x = x.__code__
    elif hasattr(x, 'gi_code'):  # ...a generator object, or
        x = x.gi_code
    elif hasattr(x, 'ag_code'):  # ...an asynchronous generator object, or
        x = x.ag_code
    elif hasattr(x, 'cr_code'):  # ...a coroutine.
        x = x.cr_code
    # Handle source code.
    if isinstance(x, str):
        x = _try_compile(x, "<disassembly>")
    # By now, if we don't have a code object, we can't disassemble x.
    if hasattr(x, 'co_code'):
        return x
    raise TypeError(
        "don't know how to disassemble %s objects" % type(x).__name__
    )


def _deoptop(op):
    name = _all_opname[op]
    return _all_opmap[deoptmap[name]] if name in deoptmap else op


def _get_code_array(co, adaptive):
    if adaptive and hasattr(co, '_co_code_adaptive'):
        return co._co_code_adaptive
    return co.co_code


def code_info(x):
    """Formatted details of methods, functions, or code."""
    return _format_code_info(_get_code_object(x))


def _format_code_info(co):
    lines = []
    lines.append("Name:              %s" % co.co_name)
    lines.append("Filename:          %s" % co.co_filename)
    lines.append("Argument count:    %s" % co.co_argcount)
    lines.append("Positional-only arguments: %s" % co.co_posonlyargcount)
    lines.append("Kw-only arguments: %s" % co.co_kwonlyargcount)
    lines.append("Number of locals:  %s" % co.co_nlocals)
    lines.append("Stack size:        %s" % co.co_stacksize)
    lines.append("Flags:             %s" % pretty_flags(co.co_flags))
    if co.co_consts:
        lines.append("Constants:")
        for i_c in enumerate(co.co_consts):
            lines.append("%4d: %r" % i_c)
    if co.co_names:
        lines.append("Names:")
        for i_n in enumerate(co.co_names):
            lines.append("%4d: %s" % i_n)
    if co.co_varnames:
        lines.append("Variable names:")
        for i_n in enumerate(co.co_varnames):
            lines.append("%4d: %s" % i_n)
    if co.co_freevars:
        lines.append("Free variables:")
        for i_n in enumerate(co.co_freevars):
            lines.append("%4d: %s" % i_n)
    if co.co_cellvars:
        lines.append("Cell variables:")
        for i_n in enumerate(co.co_cellvars):
            lines.append("%4d: %s" % i_n)
    return "\n".join(lines)


def show_code(co, *, file=None):
    """Print details of methods, functions, or code to *file*.

    If *file* is not provided, the output is printed on stdout.
    """
    print(code_info(co), file=file)


Positions = collections.namedtuple(
    'Positions',
    [
        'lineno',
        'end_lineno',
        'col_offset',
        'end_col_offset',
    ],
    defaults=[None] * 4,
)

_Instruction = collections.namedtuple(
    "_Instruction",
    [
        'opname',
        'opcode',
        'arg',
        'argval',
        'argrepr',
        'offset',
        'starts_line',
        'is_jump_target',
        'positions',
    ],
    defaults=[None],
)

_ExceptionTableEntry = collections.namedtuple(
    "_ExceptionTableEntry",
    "start end target depth lasti",
)

_OPNAME_WIDTH = 20
_OPARG_WIDTH = 5


class Instruction(_Instruction):
    """Details for a bytecode operation

       Defined fields:
         opname - human readable name for operation
         opcode - numeric code for operation
         arg - numeric argument to operation (if any), otherwise None
         argval - resolved arg value (if known), otherwise same as arg
         argrepr - human readable description of operation argument
         offset - start index of operation within bytecode sequence
         starts_line - line started by this opcode (if any), otherwise None
         is_jump_target - True if other code jumps to here, otherwise False
         positions - Optional dis.Positions object holding the span of source code
                     covered by this instruction
    """

    def _disassemble(self, lineno_width=3, mark_as_current=False, offset_width=4):
        """Format instruction details for inclusion in disassembly output

        *lineno_width* sets the width of the line number field (0 omits it)
        *mark_as_current* inserts a '-->' marker arrow as part of the line
        *offset_width* sets the width of the instruction offset field
        """
        fields = []
        # Column: Source code line number
        if lineno_width:
            if self.starts_line is not None:
                lineno_fmt = "%%%dd" % lineno_width
                fields.append(lineno_fmt % self.starts_line)
            else:
                fields.append(' ' * lineno_width)
        # Column: Current instruction indicator
        if mark_as_current:
            fields.append('-->')
        else:
            fields.append('   ')
        # Column: Jump target marker
        if self.is_jump_target:
            fields.append('>>')
        else:
            fields.append('  ')
        # Column: Instruction offset from start of code sequence
        fields.append(repr(self.offset).rjust(offset_width))
        # Column: Opcode name
        fields.append(self.opname.ljust(_OPNAME_WIDTH))
        # Column: Opcode argument
        if self.arg is not None:
            fields.append(repr(self.arg).rjust(_OPARG_WIDTH))
            # Column: Opcode argument details
            if self.argrepr:
                fields.append('(' + self.argrepr + ')')
        return ' '.join(fields).rstrip()


def _iter_fncode_instructions(rows, line_offset=0):
    for (
        name,
        op,
        arg,
        argval,
        argrepr,
        offset,
        starts_line,
        is_jump_target,
        pos_tuple,
    ) in rows:
        if starts_line is not None:
            starts_line = starts_line + line_offset
        lineno, end_lineno, col_offset, end_col_offset = pos_tuple
        if lineno is not None:
            lineno = lineno + line_offset
        if end_lineno is not None:
            end_lineno = end_lineno + line_offset
        yield Instruction(
            name,
            op,
            arg,
            argval,
            argrepr,
            offset,
            starts_line,
            is_jump_target,
            Positions(lineno, end_lineno, col_offset, end_col_offset),
        )


def _varname_from_oparg_for(co):
    fn = getattr(co, '_varname_from_oparg', None)
    if fn is not None:
        return fn
    combined = tuple(co.co_varnames) + tuple(co.co_cellvars) + tuple(co.co_freevars)
    return combined.__getitem__


def get_instructions(x, *, first_line=None, show_caches=False, adaptive=False):
    """Iterator for the opcodes in methods, functions or code

    Generates a series of Instruction named tuples giving the details of
    each operation in the supplied code.

    If *first_line* is not None, it indicates the line number that should
    be reported for the first source line in the disassembled code.
    Otherwise, the source line information (if any) is taken directly from
    the disassembled code object.
    """
    co = _get_code_object(x)
    if first_line is not None:
        line_offset = first_line - co.co_firstlineno
    else:
        line_offset = 0
    raw = _get_code_array(co, adaptive)
    if not raw:
        info = _dis_fncode_info(co)
        if info is not None:
            return _iter_fncode_instructions(info[0], line_offset=line_offset)
    linestarts = dict(findlinestarts(co))
    co_positions = co.co_positions() if hasattr(co, 'co_positions') else None
    return _get_instructions_bytes(
        raw,
        _varname_from_oparg_for(co),
        co.co_names,
        co.co_consts,
        linestarts,
        line_offset,
        co_positions=co_positions,
        show_caches=show_caches,
    )


def _get_const_value(op, arg, co_consts):
    assert op in hasconst
    argval = UNKNOWN
    if co_consts is not None:
        argval = co_consts[arg]
    return argval


def _get_const_info(op, arg, co_consts):
    argval = _get_const_value(op, arg, co_consts)
    argrepr = repr(argval) if argval is not UNKNOWN else ''
    return argval, argrepr


def _get_name_info(name_index, get_name, **extrainfo):
    if get_name is not None:
        argval = get_name(name_index, **extrainfo)
        return argval, argval
    return UNKNOWN, ''


def _parse_varint(iterator):
    b = next(iterator)
    val = b & 63
    while b & 64:
        val <<= 6
        b = next(iterator)
        val |= b & 63
    return val


def _parse_exception_table(code):
    raw_table = getattr(code, 'co_exceptiontable', None)
    if not raw_table:
        return []
    iterator = iter(raw_table)
    entries = []
    try:
        while True:
            start = _parse_varint(iterator) * 2
            length = _parse_varint(iterator) * 2
            end = start + length
            target = _parse_varint(iterator) * 2
            dl = _parse_varint(iterator)
            depth = dl >> 1
            lasti = bool(dl & 1)
            entries.append(_ExceptionTableEntry(start, end, target, depth, lasti))
    except StopIteration:
        return entries


def _is_backward_jump(op):
    return 'JUMP_BACKWARD' in opname[op]


def _get_instructions_bytes(
    code,
    varname_from_oparg=None,
    names=None,
    co_consts=None,
    linestarts=None,
    line_offset=0,
    exception_entries=(),
    co_positions=None,
    show_caches=False,
):
    """Iterate over the instructions in a bytecode string."""
    co_positions = co_positions or iter(())
    get_name = None if names is None else names.__getitem__
    labels = set(findlabels(code))
    for start, end, target, _, _ in exception_entries:
        for i in range(start, end):
            labels.add(target)
    starts_line = None
    for offset, op, arg in _unpack_opargs(code):
        if linestarts is not None:
            starts_line = linestarts.get(offset, None)
            if starts_line is not None:
                starts_line += line_offset
        is_jump_target = offset in labels
        argval = None
        argrepr = ''
        positions = Positions(*next(co_positions, ()))
        deop = _deoptop(op)
        caches = _inline_cache_entries[deop]
        if arg is not None:
            argval = arg
            if deop in hasconst:
                argval, argrepr = _get_const_info(deop, arg, co_consts)
            elif deop in hasname:
                if deop == LOAD_GLOBAL:
                    argval, argrepr = _get_name_info(arg // 2, get_name)
                    if (arg & 1) and argrepr:
                        argrepr = "NULL + " + argrepr
                elif deop == LOAD_ATTR:
                    argval, argrepr = _get_name_info(arg // 2, get_name)
                    if (arg & 1) and argrepr:
                        argrepr = "NULL|self + " + argrepr
                elif deop == LOAD_SUPER_ATTR:
                    argval, argrepr = _get_name_info(arg // 4, get_name)
                    if (arg & 1) and argrepr:
                        argrepr = "NULL|self + " + argrepr
                else:
                    argval, argrepr = _get_name_info(arg, get_name)
            elif deop in hasjabs:
                argval = arg * 2
                argrepr = "to " + repr(argval)
            elif deop in hasjrel:
                signed_arg = -arg if _is_backward_jump(deop) else arg
                argval = offset + 2 + signed_arg * 2
                argval += 2 * caches
                argrepr = "to " + repr(argval)
            elif deop in haslocal or deop in hasfree:
                argval, argrepr = _get_name_info(arg, varname_from_oparg)
            elif deop in hascompare:
                argval = cmp_op[arg >> 4]
                argrepr = argval
            elif deop == FORMAT_VALUE:
                argval, argrepr = FORMAT_VALUE_CONVERTERS[arg & 0x3]
                argval = (argval, bool(arg & 0x4))
                if argval[1]:
                    if argrepr:
                        argrepr += ', '
                    argrepr += 'with format'
            elif deop == MAKE_FUNCTION:
                argrepr = ', '.join(
                    s for i, s in enumerate(MAKE_FUNCTION_FLAGS) if arg & (1 << i)
                )
            elif deop == BINARY_OP:
                _, argrepr = _nb_ops[arg]
            elif deop == CALL_INTRINSIC_1:
                argrepr = _intrinsic_1_descs[arg]
            elif deop == CALL_INTRINSIC_2:
                argrepr = _intrinsic_2_descs[arg]
        yield Instruction(
            _all_opname[op],
            op,
            arg,
            argval,
            argrepr,
            offset,
            starts_line,
            is_jump_target,
            positions,
        )
        caches = _inline_cache_entries[deop]
        if not caches:
            continue
        if not show_caches:
            for _ in range(caches):
                next(co_positions, ())
            continue
        for name, size in _cache_format[opname[deop]].items():
            for i in range(size):
                offset += 2
                if i == 0:
                    data = code[offset: offset + 2 * size]
                    argrepr = f"{name}: {int.from_bytes(data, sys.byteorder)}"
                else:
                    argrepr = ""
                yield Instruction(
                    "CACHE",
                    CACHE,
                    0,
                    None,
                    argrepr,
                    offset,
                    None,
                    False,
                    Positions(*next(co_positions, ())),
                )


def _disassemble_fncode(
    info, lasti=-1, *, file=None, line_offset=0
):
    rows, linestarts_list, _, exc_entries = info
    linestarts = dict(linestarts_list)
    show_lineno = bool(linestarts)
    if show_lineno:
        maxlineno = max(linestarts.values()) + line_offset
        if maxlineno >= 1000:
            lineno_width = len(str(maxlineno))
        else:
            lineno_width = 3
    else:
        lineno_width = 0
    maxoffset = len(rows) - 1 if rows else 0
    if maxoffset >= 10000:
        offset_width = len(str(maxoffset))
    else:
        offset_width = 4
    for instr in _iter_fncode_instructions(rows, line_offset=line_offset):
        new_source_line = (
            show_lineno and instr.starts_line is not None and instr.offset > 0
        )
        if new_source_line:
            print(file=file)
        is_current_instr = instr.offset == lasti
        print(
            instr._disassemble(lineno_width, is_current_instr, offset_width),
            file=file,
        )
    if exc_entries:
        print("ExceptionTable:", file=file)
        for start, end, target in exc_entries:
            print(f"  {start} to {end - 1} -> {target}", file=file)


def disassemble(co, lasti=-1, *, file=None, show_caches=False, adaptive=False):
    """Disassemble a code object."""
    raw = _get_code_array(co, adaptive)
    if not raw:
        info = _dis_fncode_info(co)
        if info is not None:
            _disassemble_fncode(info, lasti=lasti, file=file)
            return
    linestarts = dict(findlinestarts(co))
    exception_entries = _parse_exception_table(co)
    co_positions = co.co_positions() if hasattr(co, 'co_positions') else None
    _disassemble_bytes(
        raw,
        lasti,
        _varname_from_oparg_for(co),
        co.co_names,
        co.co_consts,
        linestarts,
        file=file,
        exception_entries=exception_entries,
        co_positions=co_positions,
        show_caches=show_caches,
    )


def _disassemble_recursive(co, *, file=None, depth=None, show_caches=False, adaptive=False):
    disassemble(co, file=file, show_caches=show_caches, adaptive=adaptive)
    if depth is None or depth > 0:
        if depth is not None:
            depth = depth - 1
        nested = []
        for x in co.co_consts:
            if hasattr(x, 'co_code'):
                nested.append(x)
        if not nested:
            info = _dis_fncode_info(co)
            if info is not None:
                nested = info[2]
        for x in nested:
            print(file=file)
            print("Disassembly of %r:" % (x,), file=file)
            _disassemble_recursive(
                x, file=file, depth=depth, show_caches=show_caches, adaptive=adaptive
            )


def _disassemble_bytes(
    code,
    lasti=-1,
    varname_from_oparg=None,
    names=None,
    co_consts=None,
    linestarts=None,
    *,
    file=None,
    line_offset=0,
    exception_entries=(),
    co_positions=None,
    show_caches=False,
):
    show_lineno = bool(linestarts)
    if show_lineno:
        maxlineno = max(linestarts.values()) + line_offset
        if maxlineno >= 1000:
            lineno_width = len(str(maxlineno))
        else:
            lineno_width = 3
    else:
        lineno_width = 0
    maxoffset = len(code) - 2
    if maxoffset >= 10000:
        offset_width = len(str(maxoffset))
    else:
        offset_width = 4
    for instr in _get_instructions_bytes(
        code,
        varname_from_oparg,
        names,
        co_consts,
        linestarts,
        line_offset=line_offset,
        exception_entries=exception_entries,
        co_positions=co_positions,
        show_caches=show_caches,
    ):
        new_source_line = (
            show_lineno and instr.starts_line is not None and instr.offset > 0
        )
        if new_source_line:
            print(file=file)
        if show_caches:
            is_current_instr = instr.offset == lasti
        else:
            is_current_instr = (
                instr.offset
                <= lasti
                <= instr.offset + 2 * _inline_cache_entries[_deoptop(instr.opcode)]
            )
        print(
            instr._disassemble(lineno_width, is_current_instr, offset_width),
            file=file,
        )
    if exception_entries:
        print("ExceptionTable:", file=file)
        for entry in exception_entries:
            lasti_s = " lasti" if entry.lasti else ""
            end = entry.end - 2
            print(
                f"  {entry.start} to {end} -> {entry.target} [{entry.depth}]{lasti_s}",
                file=file,
            )


def _disassemble_str(source, **kwargs):
    """Compile the source string, then disassemble the code object."""
    _disassemble_recursive(_try_compile(source, '<dis>'), **kwargs)


disco = disassemble  # XXX For backwards compatibility

_INT_BITS = 32
_INT_OVERFLOW = 2 ** (_INT_BITS - 1)


def _unpack_opargs(code):
    extended_arg = 0
    caches = 0
    for i in range(0, len(code), 2):
        if caches:
            caches -= 1
            continue
        op = code[i]
        deop = _deoptop(op)
        caches = _inline_cache_entries[deop]
        if deop in hasarg:
            arg = code[i + 1] | extended_arg
            extended_arg = (arg << 8) if deop == EXTENDED_ARG else 0
            if extended_arg >= _INT_OVERFLOW:
                extended_arg -= 2 * _INT_OVERFLOW
        else:
            arg = None
            extended_arg = 0
        yield (i, op, arg)


def findlabels(code):
    """Detect all offsets in a byte code which are jump targets.

    Return the list of offsets.
    """
    labels = []
    for offset, op, arg in _unpack_opargs(code):
        if arg is not None:
            deop = _deoptop(op)
            caches = _inline_cache_entries[deop]
            if deop in hasjrel:
                if _is_backward_jump(deop):
                    arg = -arg
                label = offset + 2 + arg * 2
                label += 2 * caches
            elif deop in hasjabs:
                label = arg * 2
            else:
                continue
            if label not in labels:
                labels.append(label)
    return labels


def findlinestarts(code):
    """Find the offsets in a byte code which are start of lines in the source.

    Generate pairs (offset, lineno).
    """
    co_lines = getattr(code, 'co_lines', None)
    if co_lines is not None:
        lastline = None
        for start, end, line in co_lines():
            if line is not None and line != lastline:
                lastline = line
                yield start, line
        return
    info = _dis_fncode_info(code)
    if info is not None:
        for offset, line in info[1]:
            yield offset, line
        return


class Bytecode:
    """The bytecode operations of a piece of code

    Instantiate this with a function, method, other compiled object, string of
    code, or a code object (as returned by compile()).

    Iterating over this yields the bytecode operations as Instruction instances.
    """

    def __init__(
        self,
        x,
        *,
        first_line=None,
        current_offset=None,
        show_caches=False,
        adaptive=False,
    ):
        self.codeobj = co = _get_code_object(x)
        if first_line is None:
            self.first_line = co.co_firstlineno
            self._line_offset = 0
        else:
            self.first_line = first_line
            self._line_offset = first_line - co.co_firstlineno
        self._linestarts = dict(findlinestarts(co))
        self._original_object = x
        self.current_offset = current_offset
        self.exception_entries = _parse_exception_table(co)
        self.show_caches = show_caches
        self.adaptive = adaptive

    def __iter__(self):
        co = self.codeobj
        raw = _get_code_array(co, self.adaptive)
        if not raw:
            info = _dis_fncode_info(co)
            if info is not None:
                return _iter_fncode_instructions(
                    info[0], line_offset=self._line_offset
                )
        co_positions = co.co_positions() if hasattr(co, 'co_positions') else None
        return _get_instructions_bytes(
            raw,
            _varname_from_oparg_for(co),
            co.co_names,
            co.co_consts,
            self._linestarts,
            line_offset=self._line_offset,
            exception_entries=self.exception_entries,
            co_positions=co_positions,
            show_caches=self.show_caches,
        )

    def __repr__(self):
        return "{}({!r})".format(
            self.__class__.__name__,
            self._original_object,
        )

    @classmethod
    def from_traceback(cls, tb, *, show_caches=False, adaptive=False):
        """Construct a Bytecode from the given traceback."""
        while tb.tb_next:
            tb = tb.tb_next
        return cls(
            _tb_code_object(tb),
            current_offset=tb.tb_lasti,
            show_caches=show_caches,
            adaptive=adaptive,
        )

    def info(self):
        """Return formatted information about the code object."""
        return _format_code_info(self.codeobj)

    def dis(self):
        """Return a formatted view of the bytecode operations."""
        co = self.codeobj
        if self.current_offset is not None:
            offset = self.current_offset
        else:
            offset = -1
        with io.StringIO() as output:
            raw = _get_code_array(co, self.adaptive)
            if not raw:
                info = _dis_fncode_info(co)
                if info is not None:
                    _disassemble_fncode(
                        info,
                        lasti=offset,
                        file=output,
                        line_offset=self._line_offset,
                    )
                    return output.getvalue()
            co_positions = co.co_positions() if hasattr(co, 'co_positions') else None
            _disassemble_bytes(
                raw,
                varname_from_oparg=_varname_from_oparg_for(co),
                names=co.co_names,
                co_consts=co.co_consts,
                linestarts=self._linestarts,
                line_offset=self._line_offset,
                file=output,
                lasti=offset,
                exception_entries=self.exception_entries,
                co_positions=co_positions,
                show_caches=self.show_caches,
            )
            return output.getvalue()


def main(args=None):
    import argparse

    parser = argparse.ArgumentParser()
    parser.add_argument('infile', type=argparse.FileType('rb'), nargs='?', default='-')
    parsed = parser.parse_args(args=args)
    with parsed.infile as infile:
        source = infile.read()
    code = compile(source.decode('utf-8') if isinstance(source, bytes) else source, parsed.infile.name, "exec")
    dis(code)
