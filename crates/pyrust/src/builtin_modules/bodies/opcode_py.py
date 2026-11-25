"""
opcode module - potentially shared between dis and other modules which
operate on bytecodes (e.g. peephole optimizers).
"""

__all__ = [
    "cmp_op",
    "hasarg",
    "hasconst",
    "hasname",
    "hasjrel",
    "hasjabs",
    "haslocal",
    "hascompare",
    "hasfree",
    "hasexc",
    "opname",
    "opmap",
    "HAVE_ARGUMENT",
    "EXTENDED_ARG",
    "stack_effect",
]

cmp_op = ('<', '<=', '==', '!=', '>', '>=')

hasarg = []
hasconst = []
hasname = []
hasjrel = []
hasjabs = []
haslocal = []
hascompare = []
hasfree = []
hasexc = []

ENABLE_SPECIALIZATION = True


def is_pseudo(op):
    return op >= MIN_PSEUDO_OPCODE and op <= MAX_PSEUDO_OPCODE


oplists = [
    hasarg,
    hasconst,
    hasname,
    hasjrel,
    hasjabs,
    haslocal,
    hascompare,
    hasfree,
    hasexc,
]

opmap = {}

# pseudo opcodes (used in the compiler) mapped to the values
# they can become in the actual code.
_pseudo_ops = {}


def def_op(name, op):
    opmap[name] = op


def name_op(name, op):
    def_op(name, op)
    hasname.append(op)


def jrel_op(name, op):
    def_op(name, op)
    hasjrel.append(op)


def jabs_op(name, op):
    def_op(name, op)
    hasjabs.append(op)


def pseudo_op(name, op, real_ops):
    def_op(name, op)
    _pseudo_ops[name] = real_ops
    # add the pseudo opcode to the lists its targets are in
    for oplist in oplists:
        res = [opmap[rop] in oplist for rop in real_ops]
        if any(res):
            assert all(res)
            oplist.append(op)


# Instruction opcodes for compiled code (Python 3.12)

def_op('CACHE', 0)
def_op('POP_TOP', 1)
def_op('PUSH_NULL', 2)
def_op('INTERPRETER_EXIT', 3)

def_op('END_FOR', 4)
def_op('END_SEND', 5)

def_op('NOP', 9)

def_op('UNARY_NEGATIVE', 11)
def_op('UNARY_NOT', 12)

def_op('UNARY_INVERT', 15)

def_op('RESERVED', 17)

def_op('BINARY_SUBSCR', 25)
def_op('BINARY_SLICE', 26)
def_op('STORE_SLICE', 27)

def_op('GET_LEN', 30)
def_op('MATCH_MAPPING', 31)
def_op('MATCH_SEQUENCE', 32)
def_op('MATCH_KEYS', 33)

def_op('PUSH_EXC_INFO', 35)
def_op('CHECK_EXC_MATCH', 36)
def_op('CHECK_EG_MATCH', 37)

def_op('WITH_EXCEPT_START', 49)
def_op('GET_AITER', 50)
def_op('GET_ANEXT', 51)
def_op('BEFORE_ASYNC_WITH', 52)
def_op('BEFORE_WITH', 53)
def_op('END_ASYNC_FOR', 54)
def_op('CLEANUP_THROW', 55)

def_op('STORE_SUBSCR', 60)
def_op('DELETE_SUBSCR', 61)

def_op('GET_ITER', 68)
def_op('GET_YIELD_FROM_ITER', 69)

def_op('LOAD_BUILD_CLASS', 71)

def_op('LOAD_ASSERTION_ERROR', 74)
def_op('RETURN_GENERATOR', 75)

def_op('RETURN_VALUE', 83)

def_op('SETUP_ANNOTATIONS', 85)
def_op('LOAD_LOCALS', 87)

def_op('POP_EXCEPT', 89)

HAVE_ARGUMENT = 90             # real opcodes from here have an argument:

name_op('STORE_NAME', 90)       # Index in name list
name_op('DELETE_NAME', 91)      # ""
def_op('UNPACK_SEQUENCE', 92)   # Number of tuple items
jrel_op('FOR_ITER', 93)
def_op('UNPACK_EX', 94)
name_op('STORE_ATTR', 95)       # Index in name list
name_op('DELETE_ATTR', 96)      # ""
name_op('STORE_GLOBAL', 97)     # ""
name_op('DELETE_GLOBAL', 98)    # ""
def_op('SWAP', 99)
def_op('LOAD_CONST', 100)       # Index in const list
hasconst.append(100)
name_op('LOAD_NAME', 101)       # Index in name list
def_op('BUILD_TUPLE', 102)      # Number of tuple items
def_op('BUILD_LIST', 103)       # Number of list items
def_op('BUILD_SET', 104)        # Number of set items
def_op('BUILD_MAP', 105)        # Number of dict entries
name_op('LOAD_ATTR', 106)       # Index in name list
def_op('COMPARE_OP', 107)       # Comparison operator
hascompare.append(107)
name_op('IMPORT_NAME', 108)     # Index in name list
name_op('IMPORT_FROM', 109)     # Index in name list
jrel_op('JUMP_FORWARD', 110)    # Number of words to skip
jrel_op('POP_JUMP_IF_FALSE', 114)
jrel_op('POP_JUMP_IF_TRUE', 115)
name_op('LOAD_GLOBAL', 116)     # Index in name list
def_op('IS_OP', 117)
def_op('CONTAINS_OP', 118)
def_op('RERAISE', 119)
def_op('COPY', 120)
def_op('RETURN_CONST', 121)
hasconst.append(121)
def_op('BINARY_OP', 122)
jrel_op('SEND', 123)            # Number of words to skip
def_op('LOAD_FAST', 124)        # Local variable number, no null check
haslocal.append(124)
def_op('STORE_FAST', 125)       # Local variable number
haslocal.append(125)
def_op('DELETE_FAST', 126)      # Local variable number
haslocal.append(126)
def_op('LOAD_FAST_CHECK', 127)  # Local variable number
haslocal.append(127)
jrel_op('POP_JUMP_IF_NOT_NONE', 128)
jrel_op('POP_JUMP_IF_NONE', 129)
def_op('RAISE_VARARGS', 130)    # Number of raise arguments (1, 2, or 3)
def_op('GET_AWAITABLE', 131)
def_op('MAKE_FUNCTION', 132)    # Flags
def_op('BUILD_SLICE', 133)      # Number of items
jrel_op('JUMP_BACKWARD_NO_INTERRUPT', 134)
def_op('MAKE_CELL', 135)
hasfree.append(135)
def_op('LOAD_CLOSURE', 136)
hasfree.append(136)
def_op('LOAD_DEREF', 137)
hasfree.append(137)
def_op('STORE_DEREF', 138)
hasfree.append(138)
def_op('DELETE_DEREF', 139)
hasfree.append(139)
jrel_op('JUMP_BACKWARD', 140)
name_op('LOAD_SUPER_ATTR', 141)
def_op('CALL_FUNCTION_EX', 142)  # Flags
def_op('LOAD_FAST_AND_CLEAR', 143)
haslocal.append(143)

def_op('EXTENDED_ARG', 144)
EXTENDED_ARG = 144
def_op('LIST_APPEND', 145)
def_op('SET_ADD', 146)
def_op('MAP_ADD', 147)
hasfree.append(148)
def_op('COPY_FREE_VARS', 149)
def_op('YIELD_VALUE', 150)
def_op('RESUME', 151)
def_op('MATCH_CLASS', 152)

def_op('FORMAT_VALUE', 155)
def_op('BUILD_CONST_KEY_MAP', 156)
def_op('BUILD_STRING', 157)

def_op('LIST_EXTEND', 162)
def_op('SET_UPDATE', 163)
def_op('DICT_MERGE', 164)
def_op('DICT_UPDATE', 165)

def_op('CALL', 171)
def_op('KW_NAMES', 172)
hasconst.append(172)
def_op('CALL_INTRINSIC_1', 173)
def_op('CALL_INTRINSIC_2', 174)

name_op('LOAD_FROM_DICT_OR_GLOBALS', 175)
def_op('LOAD_FROM_DICT_OR_DEREF', 176)
hasfree.append(176)

# Instrumented instructions
MIN_INSTRUMENTED_OPCODE = 237

def_op('INSTRUMENTED_LOAD_SUPER_ATTR', 237)
def_op('INSTRUMENTED_POP_JUMP_IF_NONE', 238)
def_op('INSTRUMENTED_POP_JUMP_IF_NOT_NONE', 239)
def_op('INSTRUMENTED_RESUME', 240)
def_op('INSTRUMENTED_CALL', 241)
def_op('INSTRUMENTED_RETURN_VALUE', 242)
def_op('INSTRUMENTED_YIELD_VALUE', 243)
def_op('INSTRUMENTED_CALL_FUNCTION_EX', 244)
def_op('INSTRUMENTED_JUMP_FORWARD', 245)
def_op('INSTRUMENTED_JUMP_BACKWARD', 246)
def_op('INSTRUMENTED_RETURN_CONST', 247)
def_op('INSTRUMENTED_FOR_ITER', 248)
def_op('INSTRUMENTED_POP_JUMP_IF_FALSE', 249)
def_op('INSTRUMENTED_POP_JUMP_IF_TRUE', 250)
def_op('INSTRUMENTED_END_FOR', 251)
def_op('INSTRUMENTED_END_SEND', 252)
def_op('INSTRUMENTED_INSTRUCTION', 253)
def_op('INSTRUMENTED_LINE', 254)
# 255 is reserved

hasarg.extend([op for op in opmap.values() if op >= HAVE_ARGUMENT])

MIN_PSEUDO_OPCODE = 256

pseudo_op('SETUP_FINALLY', 256, ['NOP'])
hasexc.append(256)
pseudo_op('SETUP_CLEANUP', 257, ['NOP'])
hasexc.append(257)
pseudo_op('SETUP_WITH', 258, ['NOP'])
hasexc.append(258)
pseudo_op('POP_BLOCK', 259, ['NOP'])

pseudo_op('JUMP', 260, ['JUMP_FORWARD', 'JUMP_BACKWARD'])
pseudo_op('JUMP_NO_INTERRUPT', 261, ['JUMP_FORWARD', 'JUMP_BACKWARD_NO_INTERRUPT'])

pseudo_op('LOAD_METHOD', 262, ['LOAD_ATTR'])
pseudo_op('LOAD_SUPER_METHOD', 263, ['LOAD_SUPER_ATTR'])
pseudo_op('LOAD_ZERO_SUPER_METHOD', 264, ['LOAD_SUPER_ATTR'])
pseudo_op('LOAD_ZERO_SUPER_ATTR', 265, ['LOAD_SUPER_ATTR'])

pseudo_op('STORE_FAST_MAYBE_NULL', 266, ['STORE_FAST'])

MAX_PSEUDO_OPCODE = MIN_PSEUDO_OPCODE + len(_pseudo_ops) - 1

del def_op, name_op, jrel_op, jabs_op, pseudo_op

opname = ['<%r>' % (op,) for op in range(MAX_PSEUDO_OPCODE + 1)]
for _op, _i in opmap.items():
    opname[_i] = _op
del _op, _i

_nb_ops = [
    ("NB_ADD", "+"),
    ("NB_AND", "&"),
    ("NB_FLOOR_DIVIDE", "//"),
    ("NB_LSHIFT", "<<"),
    ("NB_MATRIX_MULTIPLY", "@"),
    ("NB_MULTIPLY", "*"),
    ("NB_REMAINDER", "%"),
    ("NB_OR", "|"),
    ("NB_POWER", "**"),
    ("NB_RSHIFT", ">>"),
    ("NB_SUBTRACT", "-"),
    ("NB_TRUE_DIVIDE", "/"),
    ("NB_XOR", "^"),
    ("NB_INPLACE_ADD", "+="),
    ("NB_INPLACE_AND", "&="),
    ("NB_INPLACE_FLOOR_DIVIDE", "//="),
    ("NB_INPLACE_LSHIFT", "<<="),
    ("NB_INPLACE_MATRIX_MULTIPLY", "@="),
    ("NB_INPLACE_MULTIPLY", "*="),
    ("NB_INPLACE_REMAINDER", "%="),
    ("NB_INPLACE_OR", "|="),
    ("NB_INPLACE_POWER", "**="),
    ("NB_INPLACE_RSHIFT", ">>="),
    ("NB_INPLACE_SUBTRACT", "-="),
    ("NB_INPLACE_TRUE_DIVIDE", "/="),
    ("NB_INPLACE_XOR", "^="),
]

_intrinsic_1_descs = [
    "INTRINSIC_1_INVALID",
    "INTRINSIC_PRINT",
    "INTRINSIC_IMPORT_STAR",
    "INTRINSIC_STOPITERATION_ERROR",
    "INTRINSIC_ASYNC_GEN_WRAP",
    "INTRINSIC_UNARY_POSITIVE",
    "INTRINSIC_LIST_TO_TUPLE",
    "INTRINSIC_TYPEVAR",
    "INTRINSIC_PARAMSPEC",
    "INTRINSIC_TYPEVARTUPLE",
    "INTRINSIC_SUBSCRIPT_GENERIC",
    "INTRINSIC_TYPEALIAS",
]

_intrinsic_2_descs = [
    "INTRINSIC_2_INVALID",
    "INTRINSIC_PREP_RERAISE_STAR",
    "INTRINSIC_TYPEVAR_WITH_BOUND",
    "INTRINSIC_TYPEVAR_WITH_CONSTRAINTS",
    "INTRINSIC_SET_FUNCTION_TYPE_PARAMS",
]

_specializations = {
    "BINARY_OP": [
        "BINARY_OP_ADD_FLOAT",
        "BINARY_OP_ADD_INT",
        "BINARY_OP_ADD_UNICODE",
        "BINARY_OP_INPLACE_ADD_UNICODE",
        "BINARY_OP_MULTIPLY_FLOAT",
        "BINARY_OP_MULTIPLY_INT",
        "BINARY_OP_SUBTRACT_FLOAT",
        "BINARY_OP_SUBTRACT_INT",
    ],
    "BINARY_SUBSCR": [
        "BINARY_SUBSCR_DICT",
        "BINARY_SUBSCR_GETITEM",
        "BINARY_SUBSCR_LIST_INT",
        "BINARY_SUBSCR_TUPLE_INT",
    ],
    "CALL": [
        "CALL_PY_EXACT_ARGS",
        "CALL_PY_WITH_DEFAULTS",
        "CALL_BOUND_METHOD_EXACT_ARGS",
        "CALL_BUILTIN_CLASS",
        "CALL_BUILTIN_FAST_WITH_KEYWORDS",
        "CALL_METHOD_DESCRIPTOR_FAST_WITH_KEYWORDS",
        "CALL_NO_KW_BUILTIN_FAST",
        "CALL_NO_KW_BUILTIN_O",
        "CALL_NO_KW_ISINSTANCE",
        "CALL_NO_KW_LEN",
        "CALL_NO_KW_LIST_APPEND",
        "CALL_NO_KW_METHOD_DESCRIPTOR_FAST",
        "CALL_NO_KW_METHOD_DESCRIPTOR_NOARGS",
        "CALL_NO_KW_METHOD_DESCRIPTOR_O",
        "CALL_NO_KW_STR_1",
        "CALL_NO_KW_TUPLE_1",
        "CALL_NO_KW_TYPE_1",
    ],
    "COMPARE_OP": [
        "COMPARE_OP_FLOAT",
        "COMPARE_OP_INT",
        "COMPARE_OP_STR",
    ],
    "FOR_ITER": [
        "FOR_ITER_LIST",
        "FOR_ITER_TUPLE",
        "FOR_ITER_RANGE",
        "FOR_ITER_GEN",
    ],
    "LOAD_SUPER_ATTR": [
        "LOAD_SUPER_ATTR_ATTR",
        "LOAD_SUPER_ATTR_METHOD",
    ],
    "LOAD_ATTR": [
        "LOAD_ATTR_CLASS",
        "LOAD_ATTR_GETATTRIBUTE_OVERRIDDEN",
        "LOAD_ATTR_INSTANCE_VALUE",
        "LOAD_ATTR_MODULE",
        "LOAD_ATTR_PROPERTY",
        "LOAD_ATTR_SLOT",
        "LOAD_ATTR_WITH_HINT",
        "LOAD_ATTR_METHOD_LAZY_DICT",
        "LOAD_ATTR_METHOD_NO_DICT",
        "LOAD_ATTR_METHOD_WITH_VALUES",
    ],
    "LOAD_CONST": [
        "LOAD_CONST__LOAD_FAST",
    ],
    "LOAD_FAST": [
        "LOAD_FAST__LOAD_CONST",
        "LOAD_FAST__LOAD_FAST",
    ],
    "LOAD_GLOBAL": [
        "LOAD_GLOBAL_BUILTIN",
        "LOAD_GLOBAL_MODULE",
    ],
    "STORE_ATTR": [
        "STORE_ATTR_INSTANCE_VALUE",
        "STORE_ATTR_SLOT",
        "STORE_ATTR_WITH_HINT",
    ],
    "STORE_FAST": [
        "STORE_FAST__LOAD_FAST",
        "STORE_FAST__STORE_FAST",
    ],
    "STORE_SUBSCR": [
        "STORE_SUBSCR_DICT",
        "STORE_SUBSCR_LIST_INT",
    ],
    "UNPACK_SEQUENCE": [
        "UNPACK_SEQUENCE_LIST",
        "UNPACK_SEQUENCE_TUPLE",
        "UNPACK_SEQUENCE_TWO_TUPLE",
    ],
    "SEND": [
        "SEND_GEN",
    ],
}

_specialized_instructions = [
    opcode for family in _specializations.values() for opcode in family
]

_cache_format = {
    "LOAD_GLOBAL": {
        "counter": 1,
        "index": 1,
        "module_keys_version": 1,
        "builtin_keys_version": 1,
    },
    "BINARY_OP": {
        "counter": 1,
    },
    "UNPACK_SEQUENCE": {
        "counter": 1,
    },
    "COMPARE_OP": {
        "counter": 1,
    },
    "BINARY_SUBSCR": {
        "counter": 1,
    },
    "FOR_ITER": {
        "counter": 1,
    },
    "LOAD_SUPER_ATTR": {
        "counter": 1,
    },
    "LOAD_ATTR": {
        "counter": 1,
        "version": 2,
        "keys_version": 2,
        "descr": 4,
    },
    "STORE_ATTR": {
        "counter": 1,
        "version": 2,
        "index": 1,
    },
    "CALL": {
        "counter": 1,
        "func_version": 2,
    },
    "STORE_SUBSCR": {
        "counter": 1,
    },
    "SEND": {
        "counter": 1,
    },
}

_inline_cache_entries = [
    sum(_cache_format.get(opname[opcode], {}).values()) for opcode in range(256)
]

# Fixed stack effects matching CPython 3.12's PyCompile_OpcodeStackEffectWithJump
_FIXED_STACK_EFFECTS = {
    0: 0,    # CACHE
    1: -1,   # POP_TOP
    2: 1,    # PUSH_NULL
    3: -1,   # INTERPRETER_EXIT
    4: -2,   # END_FOR
    5: -1,   # END_SEND
    9: 0,    # NOP
    11: 0,   # UNARY_NEGATIVE
    12: 0,   # UNARY_NOT
    15: 0,   # UNARY_INVERT
    17: 0,   # RESERVED
    25: -1,  # BINARY_SUBSCR
    26: -2,  # BINARY_SLICE
    27: -4,  # STORE_SLICE
    30: 1,   # GET_LEN
    31: 1,   # MATCH_MAPPING
    32: 1,   # MATCH_SEQUENCE
    33: 1,   # MATCH_KEYS
    35: 1,   # PUSH_EXC_INFO
    36: 0,   # CHECK_EXC_MATCH
    37: 0,   # CHECK_EG_MATCH
    49: 1,   # WITH_EXCEPT_START
    50: 0,   # GET_AITER
    51: 1,   # GET_ANEXT
    52: 1,   # BEFORE_ASYNC_WITH
    53: 1,   # BEFORE_WITH
    54: -2,  # END_ASYNC_FOR
    55: -1,  # CLEANUP_THROW
    60: -3,  # STORE_SUBSCR
    61: -2,  # DELETE_SUBSCR
    68: 0,   # GET_ITER
    69: 0,   # GET_YIELD_FROM_ITER
    71: 1,   # LOAD_BUILD_CLASS
    74: 1,   # LOAD_ASSERTION_ERROR
    75: 0,   # RETURN_GENERATOR
    83: -1,  # RETURN_VALUE
    85: 0,   # SETUP_ANNOTATIONS
    87: 1,   # LOAD_LOCALS
    89: -1,  # POP_EXCEPT
    90: -1,  # STORE_NAME
    91: 0,   # DELETE_NAME
    93: 1,   # FOR_ITER
    95: -2,  # STORE_ATTR
    96: -1,  # DELETE_ATTR
    97: -1,  # STORE_GLOBAL
    98: 0,   # DELETE_GLOBAL
    99: 0,   # SWAP
    100: 1,  # LOAD_CONST
    101: 1,  # LOAD_NAME
    107: -1, # COMPARE_OP
    108: -1, # IMPORT_NAME
    109: 1,  # IMPORT_FROM
    110: 0,  # JUMP_FORWARD
    114: -1, # POP_JUMP_IF_FALSE
    115: -1, # POP_JUMP_IF_TRUE
    117: -1, # IS_OP
    118: -1, # CONTAINS_OP
    119: -1, # RERAISE
    120: 1,  # COPY
    121: 0,  # RETURN_CONST
    122: -1, # BINARY_OP
    123: 0,  # SEND
    124: 1,  # LOAD_FAST
    125: -1, # STORE_FAST
    126: 0,  # DELETE_FAST
    127: 1,  # LOAD_FAST_CHECK
    128: -1, # POP_JUMP_IF_NOT_NONE
    129: -1, # POP_JUMP_IF_NONE
    131: 0,  # GET_AWAITABLE
    134: 0,  # JUMP_BACKWARD_NO_INTERRUPT
    135: 0,  # MAKE_CELL
    136: 1,  # LOAD_CLOSURE
    137: 1,  # LOAD_DEREF
    138: -1, # STORE_DEREF
    139: 0,  # DELETE_DEREF
    140: 0,  # JUMP_BACKWARD
    143: 1,  # LOAD_FAST_AND_CLEAR
    144: 0,  # EXTENDED_ARG
    145: -1, # LIST_APPEND
    146: -1, # SET_ADD
    147: -2, # MAP_ADD
    149: 0,  # COPY_FREE_VARS
    150: 0,  # YIELD_VALUE
    151: 0,  # RESUME
    152: -2, # MATCH_CLASS
    162: -1, # LIST_EXTEND
    163: -1, # SET_UPDATE
    164: -1, # DICT_MERGE
    165: -1, # DICT_UPDATE
    172: 0,  # KW_NAMES
    173: 0,  # CALL_INTRINSIC_1
    174: -1, # CALL_INTRINSIC_2
    175: 0,  # LOAD_FROM_DICT_OR_GLOBALS
    176: 0,  # LOAD_FROM_DICT_OR_DEREF
    238: 0,  # INSTRUMENTED_POP_JUMP_IF_NONE
    239: 0,  # INSTRUMENTED_POP_JUMP_IF_NOT_NONE
    240: 0,  # INSTRUMENTED_RESUME
    241: 0,  # INSTRUMENTED_CALL
    242: -1, # INSTRUMENTED_RETURN_VALUE
    243: 0,  # INSTRUMENTED_YIELD_VALUE
    244: 0,  # INSTRUMENTED_CALL_FUNCTION_EX
    245: 0,  # INSTRUMENTED_JUMP_FORWARD
    246: 0,  # INSTRUMENTED_JUMP_BACKWARD
    247: 0,  # INSTRUMENTED_RETURN_CONST
    248: 0,  # INSTRUMENTED_FOR_ITER
    249: 0,  # INSTRUMENTED_POP_JUMP_IF_FALSE
    250: 0,  # INSTRUMENTED_POP_JUMP_IF_TRUE
    251: -2, # INSTRUMENTED_END_FOR
    252: -1, # INSTRUMENTED_END_SEND
    253: 0,  # INSTRUMENTED_INSTRUCTION
    259: 0,  # POP_BLOCK
    260: 0,  # JUMP
    261: 0,  # JUMP_NO_INTERRUPT
    262: 1,  # LOAD_METHOD
    263: -1, # LOAD_SUPER_METHOD
    264: -1, # LOAD_ZERO_SUPER_METHOD
    265: -1, # LOAD_ZERO_SUPER_ATTR
    266: -1, # STORE_FAST_MAYBE_NULL
}

_NO_ARG_PSEUDOS = {256, 257, 258, 259}


def _to_c_int(val, name):
    if isinstance(val, int):
        return int(val)
    index = getattr(type(val), "__index__", None)
    if index is not None:
        return int(index(val))
    raise TypeError(f"'{type(val).__name__}' object cannot be interpreted as an integer")


def stack_effect(opcode, oparg=None, *, jump=None):
    """Compute the stack effect of the opcode with argument oparg."""
    op = _to_c_int(opcode, "opcode")
    if jump is not None and jump is not True and jump is not False:
        raise ValueError("stack_effect: jump must be False, True or None")

    needs_arg = op >= HAVE_ARGUMENT and op not in _NO_ARG_PSEUDOS
    if needs_arg:
        if oparg is None:
            raise ValueError(
                "stack_effect: opcode requires oparg but oparg was not specified"
            )
        arg = _to_c_int(oparg, "oparg")
    else:
        if oparg is not None:
            raise ValueError(
                "stack_effect: opcode does not permit oparg but oparg was specified"
            )
        arg = 0

    if op in _FIXED_STACK_EFFECTS:
        return _FIXED_STACK_EFFECTS[op]
    if op == 92:  # UNPACK_SEQUENCE
        return arg - 1
    if op == 94:  # UNPACK_EX
        return (arg & 0xFF) + (arg >> 8)
    if op in (102, 103, 104, 157):  # BUILD_TUPLE, BUILD_LIST, BUILD_SET, BUILD_STRING
        return 1 - arg
    if op == 105:  # BUILD_MAP
        return 1 - 2 * arg
    if op == 106:  # LOAD_ATTR
        return 1 if (arg & 1) else 0
    if op == 116:  # LOAD_GLOBAL
        return 1 + (arg & 1)
    if op == 130:  # RAISE_VARARGS
        return -arg
    if op == 132:  # MAKE_FUNCTION
        return -(
            (1 if (arg & 1) else 0)
            + (1 if (arg & 2) else 0)
            + (1 if (arg & 4) else 0)
            + (1 if (arg & 8) else 0)
        )
    if op == 133:  # BUILD_SLICE
        return -2 if arg == 3 else -1
    if op in (141, 237):  # LOAD_SUPER_ATTR, INSTRUMENTED_LOAD_SUPER_ATTR
        return (arg & 1) - 2
    if op == 142:  # CALL_FUNCTION_EX
        return -3 if (arg & 1) else -2
    if op == 155:  # FORMAT_VALUE
        return -1 if (arg & 4) else 0
    if op == 156:  # BUILD_CONST_KEY_MAP
        return -arg
    if op == 171:  # CALL
        return -1 - arg
    if op == 256:  # SETUP_FINALLY
        return 0 if jump is False else 1
    if op == 257:  # SETUP_CLEANUP
        return 0 if jump is False else 2
    if op == 258:  # SETUP_WITH
        return 0 if jump is False else 1
    raise ValueError("invalid opcode or oparg")
