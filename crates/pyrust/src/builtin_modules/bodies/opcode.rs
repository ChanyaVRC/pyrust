// `opcode` module — 100% native Rust implementation of Python 3.12's `opcode`
// module (issues #3227, #3411): the bytecode opcode map, classification lists
// (`hasarg`, `hasconst`, `hasname`, `hasjrel`, `hasjabs`, `haslocal`,
// `hascompare`, `hasfree`, `hasexc`), comparison operators (`cmp_op`),
// `HAVE_ARGUMENT`, `EXTENDED_ARG`, the internal tables `dis` consumes
// (`_nb_ops`, `_intrinsic_*_descs`, `_specializations`, `_cache_format`, ...)
// and `stack_effect()`.
//
// Every table below is a `const` copied from CPython 3.12.3's `Lib/opcode.py`;
// the module attributes are rebuilt as fresh mutable Python containers on each
// import, exactly as CPython's module body would create them.
//
// Reference: <https://docs.python.org/3/library/dis.html>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{PyIndex, PyValueRef};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value, ValueKind};

const OPCODE_DOC: &str = "\nopcode module - potentially shared between dis and other modules which\n\
operate on bytecodes (e.g. peephole optimizers).\n";

/// `opcode.cmp_op`.
const CMP_OP: [&str; 6] = ["<", "<=", "==", "!=", ">", ">="];

/// `opcode.__all__`.
const ALL: [&str; 15] = [
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
];

/// First opcode that carries an argument.
const HAVE_ARGUMENT: i64 = 90;
const EXTENDED_ARG: i64 = 144;
const MIN_INSTRUMENTED_OPCODE: i64 = 237;
const MIN_PSEUDO_OPCODE: i64 = 256;
/// `MIN_PSEUDO_OPCODE + len(_pseudo_ops) - 1`.
const MAX_PSEUDO_OPCODE: i64 = MIN_PSEUDO_OPCODE + PSEUDO_OPS.len() as i64 - 1;

/// Block-push pseudo opcodes (`SETUP_FINALLY` .. `POP_BLOCK`): their `oparg`
/// is only an internal label, so CPython's `HAS_ARG()` is false for them and
/// `stack_effect()` rejects an `oparg`.
const NO_ARG_PSEUDO_OPCODES: std::ops::RangeInclusive<i64> = 256..=259;

/// `opcode.opmap` in CPython's definition order (real opcodes, then pseudo).
const OPMAP: [(&str, u16); 140] = [
    ("CACHE", 0),
    ("POP_TOP", 1),
    ("PUSH_NULL", 2),
    ("INTERPRETER_EXIT", 3),
    ("END_FOR", 4),
    ("END_SEND", 5),
    ("NOP", 9),
    ("UNARY_NEGATIVE", 11),
    ("UNARY_NOT", 12),
    ("UNARY_INVERT", 15),
    ("RESERVED", 17),
    ("BINARY_SUBSCR", 25),
    ("BINARY_SLICE", 26),
    ("STORE_SLICE", 27),
    ("GET_LEN", 30),
    ("MATCH_MAPPING", 31),
    ("MATCH_SEQUENCE", 32),
    ("MATCH_KEYS", 33),
    ("PUSH_EXC_INFO", 35),
    ("CHECK_EXC_MATCH", 36),
    ("CHECK_EG_MATCH", 37),
    ("WITH_EXCEPT_START", 49),
    ("GET_AITER", 50),
    ("GET_ANEXT", 51),
    ("BEFORE_ASYNC_WITH", 52),
    ("BEFORE_WITH", 53),
    ("END_ASYNC_FOR", 54),
    ("CLEANUP_THROW", 55),
    ("STORE_SUBSCR", 60),
    ("DELETE_SUBSCR", 61),
    ("GET_ITER", 68),
    ("GET_YIELD_FROM_ITER", 69),
    ("LOAD_BUILD_CLASS", 71),
    ("LOAD_ASSERTION_ERROR", 74),
    ("RETURN_GENERATOR", 75),
    ("RETURN_VALUE", 83),
    ("SETUP_ANNOTATIONS", 85),
    ("LOAD_LOCALS", 87),
    ("POP_EXCEPT", 89),
    ("STORE_NAME", 90),
    ("DELETE_NAME", 91),
    ("UNPACK_SEQUENCE", 92),
    ("FOR_ITER", 93),
    ("UNPACK_EX", 94),
    ("STORE_ATTR", 95),
    ("DELETE_ATTR", 96),
    ("STORE_GLOBAL", 97),
    ("DELETE_GLOBAL", 98),
    ("SWAP", 99),
    ("LOAD_CONST", 100),
    ("LOAD_NAME", 101),
    ("BUILD_TUPLE", 102),
    ("BUILD_LIST", 103),
    ("BUILD_SET", 104),
    ("BUILD_MAP", 105),
    ("LOAD_ATTR", 106),
    ("COMPARE_OP", 107),
    ("IMPORT_NAME", 108),
    ("IMPORT_FROM", 109),
    ("JUMP_FORWARD", 110),
    ("POP_JUMP_IF_FALSE", 114),
    ("POP_JUMP_IF_TRUE", 115),
    ("LOAD_GLOBAL", 116),
    ("IS_OP", 117),
    ("CONTAINS_OP", 118),
    ("RERAISE", 119),
    ("COPY", 120),
    ("RETURN_CONST", 121),
    ("BINARY_OP", 122),
    ("SEND", 123),
    ("LOAD_FAST", 124),
    ("STORE_FAST", 125),
    ("DELETE_FAST", 126),
    ("LOAD_FAST_CHECK", 127),
    ("POP_JUMP_IF_NOT_NONE", 128),
    ("POP_JUMP_IF_NONE", 129),
    ("RAISE_VARARGS", 130),
    ("GET_AWAITABLE", 131),
    ("MAKE_FUNCTION", 132),
    ("BUILD_SLICE", 133),
    ("JUMP_BACKWARD_NO_INTERRUPT", 134),
    ("MAKE_CELL", 135),
    ("LOAD_CLOSURE", 136),
    ("LOAD_DEREF", 137),
    ("STORE_DEREF", 138),
    ("DELETE_DEREF", 139),
    ("JUMP_BACKWARD", 140),
    ("LOAD_SUPER_ATTR", 141),
    ("CALL_FUNCTION_EX", 142),
    ("LOAD_FAST_AND_CLEAR", 143),
    ("EXTENDED_ARG", 144),
    ("LIST_APPEND", 145),
    ("SET_ADD", 146),
    ("MAP_ADD", 147),
    ("COPY_FREE_VARS", 149),
    ("YIELD_VALUE", 150),
    ("RESUME", 151),
    ("MATCH_CLASS", 152),
    ("FORMAT_VALUE", 155),
    ("BUILD_CONST_KEY_MAP", 156),
    ("BUILD_STRING", 157),
    ("LIST_EXTEND", 162),
    ("SET_UPDATE", 163),
    ("DICT_MERGE", 164),
    ("DICT_UPDATE", 165),
    ("CALL", 171),
    ("KW_NAMES", 172),
    ("CALL_INTRINSIC_1", 173),
    ("CALL_INTRINSIC_2", 174),
    ("LOAD_FROM_DICT_OR_GLOBALS", 175),
    ("LOAD_FROM_DICT_OR_DEREF", 176),
    ("INSTRUMENTED_LOAD_SUPER_ATTR", 237),
    ("INSTRUMENTED_POP_JUMP_IF_NONE", 238),
    ("INSTRUMENTED_POP_JUMP_IF_NOT_NONE", 239),
    ("INSTRUMENTED_RESUME", 240),
    ("INSTRUMENTED_CALL", 241),
    ("INSTRUMENTED_RETURN_VALUE", 242),
    ("INSTRUMENTED_YIELD_VALUE", 243),
    ("INSTRUMENTED_CALL_FUNCTION_EX", 244),
    ("INSTRUMENTED_JUMP_FORWARD", 245),
    ("INSTRUMENTED_JUMP_BACKWARD", 246),
    ("INSTRUMENTED_RETURN_CONST", 247),
    ("INSTRUMENTED_FOR_ITER", 248),
    ("INSTRUMENTED_POP_JUMP_IF_FALSE", 249),
    ("INSTRUMENTED_POP_JUMP_IF_TRUE", 250),
    ("INSTRUMENTED_END_FOR", 251),
    ("INSTRUMENTED_END_SEND", 252),
    ("INSTRUMENTED_INSTRUCTION", 253),
    ("INSTRUMENTED_LINE", 254),
    ("SETUP_FINALLY", 256),
    ("SETUP_CLEANUP", 257),
    ("SETUP_WITH", 258),
    ("POP_BLOCK", 259),
    ("JUMP", 260),
    ("JUMP_NO_INTERRUPT", 261),
    ("LOAD_METHOD", 262),
    ("LOAD_SUPER_METHOD", 263),
    ("LOAD_ZERO_SUPER_METHOD", 264),
    ("LOAD_ZERO_SUPER_ATTR", 265),
    ("STORE_FAST_MAYBE_NULL", 266),
];

/// `opcode.hasconst`.
const HASCONST: [u16; 3] = [100, 121, 172];

/// `opcode.hasname`.
const HASNAME: [u16; 17] = [
    90, 91, 95, 96, 97, 98, 101, 106, 108, 109, 116, 141, 175, 262, 263, 264, 265,
];

/// `opcode.hasjrel`.
const HASJREL: [u16; 11] = [93, 110, 114, 115, 123, 128, 129, 134, 140, 260, 261];

/// `opcode.hasjabs` (empty in 3.12).
const HASJABS: [u16; 0] = [];

/// `opcode.haslocal`.
const HASLOCAL: [u16; 6] = [124, 125, 126, 127, 143, 266];

/// `opcode.hascompare`.
const HASCOMPARE: [u16; 1] = [107];

/// `opcode.hasfree` (includes CPython's historical stray `148`).
const HASFREE: [u16; 7] = [135, 136, 137, 138, 139, 148, 176];

/// `opcode.hasexc` (pseudo opcodes only).
const HASEXC: [u16; 3] = [256, 257, 258];

/// `opcode._pseudo_ops` in definition order.
const PSEUDO_OPS: [(&str, &[&str]); 11] = [
    ("SETUP_FINALLY", &["NOP"]),
    ("SETUP_CLEANUP", &["NOP"]),
    ("SETUP_WITH", &["NOP"]),
    ("POP_BLOCK", &["NOP"]),
    ("JUMP", &["JUMP_FORWARD", "JUMP_BACKWARD"]),
    (
        "JUMP_NO_INTERRUPT",
        &["JUMP_FORWARD", "JUMP_BACKWARD_NO_INTERRUPT"],
    ),
    ("LOAD_METHOD", &["LOAD_ATTR"]),
    ("LOAD_SUPER_METHOD", &["LOAD_SUPER_ATTR"]),
    ("LOAD_ZERO_SUPER_METHOD", &["LOAD_SUPER_ATTR"]),
    ("LOAD_ZERO_SUPER_ATTR", &["LOAD_SUPER_ATTR"]),
    ("STORE_FAST_MAYBE_NULL", &["STORE_FAST"]),
];

/// `opcode._nb_ops` (`BINARY_OP` argument descriptions).
const NB_OPS: [(&str, &str); 26] = [
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
];

/// `opcode._intrinsic_1_descs`.
const INTRINSIC_1_DESCS: [&str; 12] = [
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
];

/// `opcode._intrinsic_2_descs`.
const INTRINSIC_2_DESCS: [&str; 5] = [
    "INTRINSIC_2_INVALID",
    "INTRINSIC_PREP_RERAISE_STAR",
    "INTRINSIC_TYPEVAR_WITH_BOUND",
    "INTRINSIC_TYPEVAR_WITH_CONSTRAINTS",
    "INTRINSIC_SET_FUNCTION_TYPE_PARAMS",
];

/// `opcode._specializations` (family -> specialized instructions).
const SPECIALIZATIONS: [(&str, &[&str]); 15] = [
    (
        "BINARY_OP",
        &[
            "BINARY_OP_ADD_FLOAT",
            "BINARY_OP_ADD_INT",
            "BINARY_OP_ADD_UNICODE",
            "BINARY_OP_INPLACE_ADD_UNICODE",
            "BINARY_OP_MULTIPLY_FLOAT",
            "BINARY_OP_MULTIPLY_INT",
            "BINARY_OP_SUBTRACT_FLOAT",
            "BINARY_OP_SUBTRACT_INT",
        ],
    ),
    (
        "BINARY_SUBSCR",
        &[
            "BINARY_SUBSCR_DICT",
            "BINARY_SUBSCR_GETITEM",
            "BINARY_SUBSCR_LIST_INT",
            "BINARY_SUBSCR_TUPLE_INT",
        ],
    ),
    (
        "CALL",
        &[
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
    ),
    (
        "COMPARE_OP",
        &["COMPARE_OP_FLOAT", "COMPARE_OP_INT", "COMPARE_OP_STR"],
    ),
    (
        "FOR_ITER",
        &[
            "FOR_ITER_LIST",
            "FOR_ITER_TUPLE",
            "FOR_ITER_RANGE",
            "FOR_ITER_GEN",
        ],
    ),
    (
        "LOAD_SUPER_ATTR",
        &["LOAD_SUPER_ATTR_ATTR", "LOAD_SUPER_ATTR_METHOD"],
    ),
    (
        "LOAD_ATTR",
        &[
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
    ),
    ("LOAD_CONST", &["LOAD_CONST__LOAD_FAST"]),
    (
        "LOAD_FAST",
        &["LOAD_FAST__LOAD_CONST", "LOAD_FAST__LOAD_FAST"],
    ),
    (
        "LOAD_GLOBAL",
        &["LOAD_GLOBAL_BUILTIN", "LOAD_GLOBAL_MODULE"],
    ),
    (
        "STORE_ATTR",
        &[
            "STORE_ATTR_INSTANCE_VALUE",
            "STORE_ATTR_SLOT",
            "STORE_ATTR_WITH_HINT",
        ],
    ),
    (
        "STORE_FAST",
        &["STORE_FAST__LOAD_FAST", "STORE_FAST__STORE_FAST"],
    ),
    (
        "STORE_SUBSCR",
        &["STORE_SUBSCR_DICT", "STORE_SUBSCR_LIST_INT"],
    ),
    (
        "UNPACK_SEQUENCE",
        &[
            "UNPACK_SEQUENCE_LIST",
            "UNPACK_SEQUENCE_TUPLE",
            "UNPACK_SEQUENCE_TWO_TUPLE",
        ],
    ),
    ("SEND", &["SEND_GEN"]),
];

/// `opcode._cache_format` (instruction -> inline-cache entry layout).
const CACHE_FORMAT: [(&str, &[(&str, i64)]); 12] = [
    (
        "LOAD_GLOBAL",
        &[
            ("counter", 1),
            ("index", 1),
            ("module_keys_version", 1),
            ("builtin_keys_version", 1),
        ],
    ),
    ("BINARY_OP", &[("counter", 1)]),
    ("UNPACK_SEQUENCE", &[("counter", 1)]),
    ("COMPARE_OP", &[("counter", 1)]),
    ("BINARY_SUBSCR", &[("counter", 1)]),
    ("FOR_ITER", &[("counter", 1)]),
    ("LOAD_SUPER_ATTR", &[("counter", 1)]),
    (
        "LOAD_ATTR",
        &[
            ("counter", 1),
            ("version", 2),
            ("keys_version", 2),
            ("descr", 4),
        ],
    ),
    (
        "STORE_ATTR",
        &[("counter", 1), ("version", 2), ("index", 1)],
    ),
    ("CALL", &[("counter", 1), ("func_version", 2)]),
    ("STORE_SUBSCR", &[("counter", 1)]),
    ("SEND", &[("counter", 1)]),
];

/// Opcodes whose stack effect does not depend on `oparg` or `jump`
/// (CPython 3.12 `stack_effect()` in `Python/compile.c`), sorted by opcode.
const FIXED_STACK_EFFECTS: [(u16, i8); 111] = [
    (0, 0),    // CACHE
    (1, -1),   // POP_TOP
    (2, 1),    // PUSH_NULL
    (3, -1),   // INTERPRETER_EXIT
    (4, -2),   // END_FOR
    (5, -1),   // END_SEND
    (9, 0),    // NOP
    (11, 0),   // UNARY_NEGATIVE
    (12, 0),   // UNARY_NOT
    (15, 0),   // UNARY_INVERT
    (17, 0),   // RESERVED
    (25, -1),  // BINARY_SUBSCR
    (26, -2),  // BINARY_SLICE
    (27, -4),  // STORE_SLICE
    (30, 1),   // GET_LEN
    (31, 1),   // MATCH_MAPPING
    (32, 1),   // MATCH_SEQUENCE
    (33, 1),   // MATCH_KEYS
    (35, 1),   // PUSH_EXC_INFO
    (36, 0),   // CHECK_EXC_MATCH
    (37, 0),   // CHECK_EG_MATCH
    (49, 1),   // WITH_EXCEPT_START
    (50, 0),   // GET_AITER
    (51, 1),   // GET_ANEXT
    (52, 1),   // BEFORE_ASYNC_WITH
    (53, 1),   // BEFORE_WITH
    (54, -2),  // END_ASYNC_FOR
    (55, -1),  // CLEANUP_THROW
    (60, -3),  // STORE_SUBSCR
    (61, -2),  // DELETE_SUBSCR
    (68, 0),   // GET_ITER
    (69, 0),   // GET_YIELD_FROM_ITER
    (71, 1),   // LOAD_BUILD_CLASS
    (74, 1),   // LOAD_ASSERTION_ERROR
    (75, 0),   // RETURN_GENERATOR
    (83, -1),  // RETURN_VALUE
    (85, 0),   // SETUP_ANNOTATIONS
    (87, 1),   // LOAD_LOCALS
    (89, -1),  // POP_EXCEPT
    (90, -1),  // STORE_NAME
    (91, 0),   // DELETE_NAME
    (93, 1),   // FOR_ITER
    (95, -2),  // STORE_ATTR
    (96, -1),  // DELETE_ATTR
    (97, -1),  // STORE_GLOBAL
    (98, 0),   // DELETE_GLOBAL
    (100, 1),  // LOAD_CONST
    (101, 1),  // LOAD_NAME
    (107, -1), // COMPARE_OP
    (108, -1), // IMPORT_NAME
    (109, 1),  // IMPORT_FROM
    (110, 0),  // JUMP_FORWARD
    (114, -1), // POP_JUMP_IF_FALSE
    (115, -1), // POP_JUMP_IF_TRUE
    (117, -1), // IS_OP
    (118, -1), // CONTAINS_OP
    (121, 0),  // RETURN_CONST
    (122, -1), // BINARY_OP
    (123, 0),  // SEND
    (124, 1),  // LOAD_FAST
    (125, -1), // STORE_FAST
    (126, 0),  // DELETE_FAST
    (127, 1),  // LOAD_FAST_CHECK
    (128, -1), // POP_JUMP_IF_NOT_NONE
    (129, -1), // POP_JUMP_IF_NONE
    (131, 0),  // GET_AWAITABLE
    (134, 0),  // JUMP_BACKWARD_NO_INTERRUPT
    (135, 0),  // MAKE_CELL
    (136, 1),  // LOAD_CLOSURE
    (137, 1),  // LOAD_DEREF
    (138, -1), // STORE_DEREF
    (139, 0),  // DELETE_DEREF
    (140, 0),  // JUMP_BACKWARD
    (143, 1),  // LOAD_FAST_AND_CLEAR
    (144, 0),  // EXTENDED_ARG
    (147, -2), // MAP_ADD
    (149, 0),  // COPY_FREE_VARS
    (150, 0),  // YIELD_VALUE
    (151, 0),  // RESUME
    (152, -2), // MATCH_CLASS
    (164, -1), // DICT_MERGE
    (165, -1), // DICT_UPDATE
    (172, 0),  // KW_NAMES
    (173, 0),  // CALL_INTRINSIC_1
    (174, -1), // CALL_INTRINSIC_2
    (175, 0),  // LOAD_FROM_DICT_OR_GLOBALS
    (176, 0),  // LOAD_FROM_DICT_OR_DEREF
    (238, 0),  // INSTRUMENTED_POP_JUMP_IF_NONE
    (239, 0),  // INSTRUMENTED_POP_JUMP_IF_NOT_NONE
    (240, 0),  // INSTRUMENTED_RESUME
    (241, 0),  // INSTRUMENTED_CALL
    (242, -1), // INSTRUMENTED_RETURN_VALUE
    (243, 0),  // INSTRUMENTED_YIELD_VALUE
    (244, 0),  // INSTRUMENTED_CALL_FUNCTION_EX
    (245, 0),  // INSTRUMENTED_JUMP_FORWARD
    (246, 0),  // INSTRUMENTED_JUMP_BACKWARD
    (247, 0),  // INSTRUMENTED_RETURN_CONST
    (248, 0),  // INSTRUMENTED_FOR_ITER
    (249, 0),  // INSTRUMENTED_POP_JUMP_IF_FALSE
    (250, 0),  // INSTRUMENTED_POP_JUMP_IF_TRUE
    (251, -2), // INSTRUMENTED_END_FOR
    (252, -1), // INSTRUMENTED_END_SEND
    (253, 0),  // INSTRUMENTED_INSTRUCTION
    (259, 0),  // POP_BLOCK
    (260, 0),  // JUMP
    (261, 0),  // JUMP_NO_INTERRUPT
    (262, 1),  // LOAD_METHOD
    (263, -1), // LOAD_SUPER_METHOD
    (264, -1), // LOAD_ZERO_SUPER_METHOD
    (265, -1), // LOAD_ZERO_SUPER_ATTR
    (266, -1), // STORE_FAST_MAYBE_NULL
];
fn int_list(items: &[u16]) -> Value {
    Value::list(items.iter().map(|&n| Value::int(i64::from(n))).collect())
}

fn str_list(items: &[&str]) -> Value {
    Value::list(items.iter().map(|s| Value::string(*s)).collect())
}

fn dict_of(entries: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut dict = PyDict::default();
    for (key, value) in entries {
        dict.insert(PyKey::str_from(key), value);
    }
    Value::dict(dict)
}

fn opname_of(code: i64) -> Option<&'static str> {
    OPMAP
        .iter()
        .find(|&&(_, c)| i64::from(c) == code)
        .map(|&(name, _)| name)
}

/// `opcode.opmap`.
fn build_opmap() -> Value {
    dict_of(
        OPMAP
            .iter()
            .map(|&(name, code)| (name, Value::int(i64::from(code)))),
    )
}

/// `opcode.opname`: `'<N>'` placeholders for unassigned codes.
fn build_opname() -> Value {
    Value::list(
        (0..=MAX_PSEUDO_OPCODE)
            .map(|code| match opname_of(code) {
                Some(name) => Value::string(name),
                None => Value::string(format!("<{code}>")),
            })
            .collect(),
    )
}

/// `opcode.hasarg`: every real opcode `>= HAVE_ARGUMENT`, then the pseudo
/// opcodes that stand for one of those (`JUMP` .. `STORE_FAST_MAYBE_NULL`).
fn build_hasarg() -> Value {
    let codes: Vec<u16> = OPMAP
        .iter()
        .map(|&(_, code)| code)
        .filter(|&code| has_arg(i64::from(code)))
        .collect();
    int_list(&codes)
}

fn build_pseudo_ops() -> Value {
    dict_of(
        PSEUDO_OPS
            .iter()
            .map(|&(name, real)| (name, str_list(real))),
    )
}

fn build_nb_ops() -> Value {
    Value::list(
        NB_OPS
            .iter()
            .map(|&(name, sym)| Value::tuple(vec![Value::string(name), Value::string(sym)]))
            .collect(),
    )
}

fn build_specializations() -> Value {
    dict_of(
        SPECIALIZATIONS
            .iter()
            .map(|&(family, ops)| (family, str_list(ops))),
    )
}

fn build_specialized_instructions() -> Value {
    Value::list(
        SPECIALIZATIONS
            .iter()
            .flat_map(|&(_, ops)| ops.iter().map(|s| Value::string(*s)))
            .collect(),
    )
}

fn build_cache_format() -> Value {
    dict_of(CACHE_FORMAT.iter().map(|&(name, fields)| {
        (
            name,
            dict_of(fields.iter().map(|&(field, n)| (field, Value::int(n)))),
        )
    }))
}

/// `opcode._inline_cache_entries`: cache-slot count for each of `0..256`.
fn build_inline_cache_entries() -> Value {
    Value::list(
        (0..256)
            .map(|code| {
                let name = opname_of(code);
                let entries: i64 = CACHE_FORMAT
                    .iter()
                    .filter(|&&(family, _)| Some(family) == name)
                    .flat_map(|&(_, fields)| fields.iter().map(|&(_, n)| n))
                    .sum();
                Value::int(entries)
            })
            .collect(),
    )
}

/// `oplists` holds the *same* list objects as the `has*` module attributes,
/// so `opcode.oplists[0] is opcode.hasarg` holds as it does in CPython.
pub(crate) fn prepare_module(_interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(module_rc) = module.kind() else {
        return;
    };
    let lists: Vec<Value> = {
        let m = module_rc.borrow();
        [
            "hasarg",
            "hasconst",
            "hasname",
            "hasjrel",
            "hasjabs",
            "haslocal",
            "hascompare",
            "hasfree",
            "hasexc",
        ]
        .iter()
        .filter_map(|name| m.attrs.get(*name).cloned())
        .collect()
    };
    module_rc
        .borrow_mut()
        .insert_attr("oplists".to_string(), Value::list(lists));
}

/// `op >= MIN_PSEUDO_OPCODE and op <= MAX_PSEUDO_OPCODE` for the numeric
/// types `opcode.is_pseudo` is called with.
fn in_pseudo_range(op: &Value) -> Result<bool> {
    let (lo, hi) = (MIN_PSEUDO_OPCODE, MAX_PSEUDO_OPCODE);
    match op.kind() {
        ValueKind::Int(n) => Ok((lo..=hi).contains(&n)),
        ValueKind::Bool(_) | ValueKind::BigInt(_) => Ok(false),
        ValueKind::Float(f) => Ok(f >= lo as f64 && f <= hi as f64),
        _ => Err(PyError::named(
            "TypeError",
            format!(
                "'>=' not supported between instances of '{}' and 'int'",
                pyrust_core::builtin_type_name(op)
            ),
        )),
    }
}

/// `HAS_ARG()` from CPython 3.12's `pycore_opcode_utils.h` as seen through
/// `opcode.hasarg`/`stack_effect()`: real opcodes `>= HAVE_ARGUMENT` plus the
/// pseudo opcodes after `POP_BLOCK`.
fn has_arg(op: i64) -> bool {
    op >= HAVE_ARGUMENT && !NO_ARG_PSEUDO_OPCODES.contains(&op)
}

/// `(popped, pushed)` stack slots of the opcodes whose layout depends on
/// `oparg`, in C `int` arithmetic exactly as CPython 3.12's generated
/// `_PyOpcode_num_popped` / `_PyOpcode_num_pushed` compute them (so a negative
/// or overflowing `oparg` yields a negative count, i.e. an invalid opcode).
fn oparg_slot_counts(op: i32, oparg: i32, taken: bool) -> Option<(i32, i32)> {
    let n = oparg;
    let flag = |bit: i32| i32::from(n & bit != 0);
    Some(match op {
        // SWAP
        99 => (n, n),
        // UNPACK_SEQUENCE
        92 => (1, n),
        // UNPACK_EX
        94 => (1, (n & 0xFF).wrapping_add(n >> 8).wrapping_add(1)),
        // BUILD_TUPLE, BUILD_LIST, BUILD_SET, BUILD_STRING
        102 | 103 | 104 | 157 => (n, 1),
        // BUILD_MAP
        105 => (n.wrapping_mul(2), 1),
        // LOAD_ATTR
        106 => (1, 1 + flag(1)),
        // LOAD_GLOBAL
        116 => (0, 1 + flag(1)),
        // RERAISE
        119 => (n.wrapping_add(1), n),
        // COPY
        120 => (n, n.wrapping_add(1)),
        // RAISE_VARARGS
        130 => (n, 0),
        // MAKE_FUNCTION
        132 => (1 + flag(1) + flag(2) + flag(4) + flag(8), 1),
        // BUILD_SLICE
        133 => (if n == 3 { 3 } else { 2 }, 1),
        // LOAD_SUPER_ATTR, INSTRUMENTED_LOAD_SUPER_ATTR
        141 | 237 => (3, 1 + flag(1)),
        // CALL_FUNCTION_EX
        142 => (3 + flag(1), 1),
        // LIST_APPEND, SET_ADD, LIST_EXTEND, SET_UPDATE
        145 | 146 | 162 | 163 => (n.wrapping_add(1), n),
        // FORMAT_VALUE
        155 => (1 + flag(4), 1),
        // BUILD_CONST_KEY_MAP
        156 => (n.wrapping_add(1), 1),
        // CALL
        171 => (n.wrapping_add(2), 1),
        // SETUP_FINALLY, SETUP_WITH
        256 | 258 => (0, i32::from(taken)),
        // SETUP_CLEANUP
        257 => (0, 2 * i32::from(taken)),
        _ => return None,
    })
}

/// `PyCompile_OpcodeStackEffectWithJump` for CPython 3.12. `jump` is the
/// `jump` keyword of `stack_effect()`; `None` selects the larger effect.
/// Returns `None` for an invalid opcode or `oparg`.
fn stack_effect_of(op: i32, oparg: i32, jump: Option<bool>) -> Option<i64> {
    if let Some((popped, pushed)) = oparg_slot_counts(op, oparg, jump != Some(false)) {
        return (popped >= 0 && pushed >= 0).then(|| i64::from(pushed) - i64::from(popped));
    }
    let idx = FIXED_STACK_EFFECTS
        .binary_search_by_key(&op, |&(code, _)| i32::from(code))
        .ok()?;
    Some(i64::from(FIXED_STACK_EFFECTS[idx].1))
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(OPCODE_DOC),
        "__all__" => str_list(&ALL),
        "cmp_op" => Value::tuple(CMP_OP.iter().map(|s| Value::string(*s)).collect()),
        "hasarg" => build_hasarg(),
        "hasconst" => int_list(&HASCONST),
        "hasname" => int_list(&HASNAME),
        "hasjrel" => int_list(&HASJREL),
        "hasjabs" => int_list(&HASJABS),
        "haslocal" => int_list(&HASLOCAL),
        "hascompare" => int_list(&HASCOMPARE),
        "hasfree" => int_list(&HASFREE),
        "hasexc" => int_list(&HASEXC),
        "ENABLE_SPECIALIZATION" => Value::bool_(true),
        "opmap" => build_opmap(),
        "_pseudo_ops" => build_pseudo_ops(),
        "HAVE_ARGUMENT" => Value::int(HAVE_ARGUMENT),
        "EXTENDED_ARG" => Value::int(EXTENDED_ARG),
        "MIN_INSTRUMENTED_OPCODE" => Value::int(MIN_INSTRUMENTED_OPCODE),
        "MIN_PSEUDO_OPCODE" => Value::int(MIN_PSEUDO_OPCODE),
        "MAX_PSEUDO_OPCODE" => Value::int(MAX_PSEUDO_OPCODE),
        "opname" => build_opname(),
        "op" => Value::string("STORE_FAST_MAYBE_NULL"),
        "i" => Value::int(MAX_PSEUDO_OPCODE),
        "_nb_ops" => build_nb_ops(),
        "_intrinsic_1_descs" => str_list(&INTRINSIC_1_DESCS),
        "_intrinsic_2_descs" => str_list(&INTRINSIC_2_DESCS),
        "_specializations" => build_specializations(),
        "_specialized_instructions" => build_specialized_instructions(),
        "_cache_format" => build_cache_format(),
        "_inline_cache_entries" => build_inline_cache_entries(),
    }

    fn is_pseudo(op: PyValueRef<'_>) -> Result<Value> {
        Ok(Value::bool_(in_pseudo_range(op.0)?))
    }

    /// Compute the stack effect of the opcode.
    ///
    /// Signature `stack_effect(opcode, oparg=None, /, *, jump=None)`; parsed
    /// by hand so the argument-count/keyword errors match CPython's
    /// Argument-Clinic wording.
    fn stack_effect(
        #[varargs] args: &[Value],
        #[varkwargs] kwargs: indexmap::IndexMap<String, Value>,
    ) -> Result<Value> {
        if args.is_empty() {
            return Err(PyError::named(
                "TypeError",
                "stack_effect() takes at least 1 positional argument (0 given)".to_string(),
            ));
        }
        if args.len() > 2 {
            return Err(PyError::named(
                "TypeError",
                format!(
                    "stack_effect() takes at most 2 positional arguments ({} given)",
                    args.len()
                ),
            ));
        }
        let mut jump = None;
        for (key, value) in &kwargs {
            if key == "jump" {
                jump = Some(value);
            } else {
                return Err(PyError::named(
                    "TypeError",
                    format!("'{key}' is an invalid keyword argument for stack_effect()"),
                ));
            }
        }
        let opcode = PyIndex::try_from_value_with_interp(_interp, &args[0], FN_NAME, "opcode")?;
        let op = opcode
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(|| {
                PyError::named(
                    "OverflowError",
                    "Python int too large to convert to C int".to_string(),
                )
            })?;
        let oparg = args.get(1).filter(|v| !v.is_none());
        let mut arg = 0i32;
        if has_arg(i64::from(op)) {
            let Some(raw) = oparg else {
                return Err(value_error(
                    "stack_effect: opcode requires oparg but oparg was not specified".into(),
                ));
            };
            let index = PyIndex::try_from_value_with_interp(_interp, raw, FN_NAME, "oparg")?;
            let wide = index.as_i64().ok_or_else(|| {
                PyError::named(
                    "OverflowError",
                    "Python int too large to convert to C long".to_string(),
                )
            })?;
            // CPython narrows the `long` to a C `int`.
            arg = wide as i32;
        } else if oparg.is_some() {
            return Err(value_error(
                "stack_effect: opcode does not permit oparg but oparg was specified".into(),
            ));
        }
        let jump = match jump.filter(|v| !v.is_none()) {
            None => None,
            Some(v) => match v.kind() {
                ValueKind::Bool(b) => Some(b),
                _ => {
                    return Err(value_error(
                        "stack_effect: jump must be False, True or None".into(),
                    ));
                }
            },
        };
        match stack_effect_of(op, arg, jump) {
            Some(effect) => Ok(Value::int(effect)),
            None => Err(value_error("invalid opcode or oparg".into())),
        }
    }
}

fn value_error(msg: String) -> PyError {
    PyError::named("ValueError", msg)
}
