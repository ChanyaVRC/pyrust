// `ast` module — body for the `ast @inject` entry in `pyrust_builtin_modules!`
// (issue #3204).
//
// Native Python 3.12 parser (`ast/parser.rs` + `ast/literals.rs`, backed by
// the shared tokenizer in `tokenize/lexer.rs`) constructs genuine Python 3.12
// `ast.AST` instances with exact `(lineno, col_offset, end_lineno,
// end_col_offset)` spans.  The full class hierarchy and utility functions
// (`parse`, `literal_eval`, `dump`, `walk`, `iter_fields`, `iter_child_nodes`,
// `NodeVisitor`, `NodeTransformer`, `fix_missing_locations`,
// `increment_lineno`, `copy_location`, `get_docstring`, `get_source_segment`,
// `unparse`) live in `ast_py.py` and are injected by `inject_python_members`.
//
// Reference: <https://docs.python.org/3.12/library/ast.html>

use std::cell::RefCell;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::PyValue;
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyKey, Value, ValueKind};

#[path = "ast/literals.rs"]
mod literals;
#[path = "ast/parser.rs"]
mod parser;

use self::parser::{AstClasses, parse_python_ast};

/// Python-source definitions for the `ast` module.
const AST_PY_SOURCE: &str = include_str!("ast_py.py");

/// Private native helper moved into `AST_PY_SOURCE`'s namespace and hidden
/// from `dir(ast)` after injection.
const AST_PRIVATE_HELPERS: &[&str] = &["_parse_to_ast"];

/// Public names exported from `AST_PY_SOURCE` onto the `ast` module.
const AST_PY_EXPORTS: &[&str] = &[
    // Compiler flags
    "PyCF_ONLY_AST",
    "PyCF_TYPE_COMMENTS",
    "PyCF_ALLOW_TOP_LEVEL_AWAIT",
    // Base & abstract node classes
    "AST",
    "mod",
    "stmt",
    "expr",
    "expr_context",
    "boolop",
    "operator",
    "unaryop",
    "cmpop",
    "comprehension",
    "excepthandler",
    "arguments",
    "arg",
    "keyword",
    "alias",
    "withitem",
    "match_case",
    "pattern",
    "type_ignore",
    "type_param",
    "slice",
    // Module root nodes
    "Module",
    "Interactive",
    "Expression",
    "FunctionType",
    "Suite",
    // Statement nodes
    "FunctionDef",
    "AsyncFunctionDef",
    "ClassDef",
    "Return",
    "Delete",
    "Assign",
    "TypeAlias",
    "AugAssign",
    "AnnAssign",
    "For",
    "AsyncFor",
    "While",
    "If",
    "With",
    "AsyncWith",
    "Match",
    "Raise",
    "Try",
    "TryStar",
    "Assert",
    "Import",
    "ImportFrom",
    "Global",
    "Nonlocal",
    "Expr",
    "Pass",
    "Break",
    "Continue",
    // Expression nodes
    "BoolOp",
    "NamedExpr",
    "BinOp",
    "UnaryOp",
    "Lambda",
    "IfExp",
    "Dict",
    "Set",
    "ListComp",
    "SetComp",
    "DictComp",
    "GeneratorExp",
    "Await",
    "Yield",
    "YieldFrom",
    "Compare",
    "Call",
    "FormattedValue",
    "JoinedStr",
    "Constant",
    "Attribute",
    "Subscript",
    "Starred",
    "Name",
    "List",
    "Tuple",
    "Slice",
    // Context nodes
    "Load",
    "Store",
    "Del",
    "AugLoad",
    "AugStore",
    "Param",
    // BoolOp nodes
    "And",
    "Or",
    // Operator nodes
    "Add",
    "Sub",
    "Mult",
    "MatMult",
    "Div",
    "Mod",
    "Pow",
    "LShift",
    "RShift",
    "BitOr",
    "BitXor",
    "BitAnd",
    "FloorDiv",
    // UnaryOp nodes
    "Invert",
    "Not",
    "UAdd",
    "USub",
    // CmpOp nodes
    "Eq",
    "NotEq",
    "Lt",
    "LtE",
    "Gt",
    "GtE",
    "Is",
    "IsNot",
    "In",
    "NotIn",
    // ExceptHandler & TypeIgnore
    "ExceptHandler",
    "TypeIgnore",
    // Pattern nodes
    "MatchValue",
    "MatchSingleton",
    "MatchSequence",
    "MatchMapping",
    "MatchClass",
    "MatchStar",
    "MatchAs",
    "MatchOr",
    // Type parameter nodes (PEP 695)
    "TypeVar",
    "ParamSpec",
    "TypeVarTuple",
    // Legacy slice & constant aliases (CPython 3.12 compatibility)
    "Index",
    "ExtSlice",
    "Num",
    "Str",
    "Bytes",
    "NameConstant",
    "Ellipsis",
    // Functions & visitor classes
    "parse",
    "literal_eval",
    "dump",
    "copy_location",
    "fix_missing_locations",
    "increment_lineno",
    "iter_fields",
    "iter_child_nodes",
    "get_docstring",
    "get_source_segment",
    "walk",
    "NodeVisitor",
    "NodeTransformer",
    "unparse",
    "main",
];

pyrust_module! {
    #[arity_style(clinic)]
    fn _parse_to_ast(
        #[positional_only] source: PyValue,
        #[positional_only] mode: PyValue,
        #[positional_only] type_comments: PyValue,
        #[positional_only] feature_minor: PyValue,
        #[positional_only] class_table: PyValue,
    ) -> Result<Value> {
        let src_str = source.0.as_str().ok_or_else(|| {
            PyError::named("TypeError", "parse() source must be str".to_string())
        })?;
        let mode_str = mode.0.as_str().ok_or_else(|| {
            PyError::named("TypeError", "parse() mode must be str".to_string())
        })?;
        let tc = type_comments.0.truthy_raw();
        let minor = feature_minor.0.as_int().unwrap_or(12).clamp(0, 99) as u32;
        let classes = AstClasses::from_dict(&class_table.0)
            .map_err(PyError::runtime)?;

        match parse_python_ast(src_str, mode_str, tc, minor, &classes) {
            Ok(node) => Ok(Value::tuple(vec![Value::bool_(true), node])),
            Err(err) => Ok(Value::tuple(vec![
                Value::bool_(false),
                Value::string(err.kind),
                Value::string(err.msg),
                Value::int(err.lineno as i64),
                Value::int(err.offset as i64),
                Value::string(err.text),
                Value::int(err.end_lineno as i64),
                Value::int(err.end_offset as i64),
            ])),
        }
    }
}

/// Execute `AST_PY_SOURCE` once, wire its `parse()` to `_parse_to_ast`, hide
/// private helpers from `dir(ast)`, and publish the CPython 3.12 `ast` surface.
pub(crate) fn inject_python_members(
    interp: &mut Interpreter,
    module: &Rc<RefCell<crate::value::PyModule>>,
) -> Result<Option<Value>> {
    let ns = crate::builtin_modules::make_module_exec_ns(module)?;
    {
        let mut module_ref = module.borrow_mut();
        for &helper in AST_PRIVATE_HELPERS {
            if let Some(func) = module_ref.attrs.shift_remove(helper) {
                ns.dict_insert(PyKey::str_from(helper), func)?;
            }
        }
    }
    interp.exec_source_with_filename(AST_PY_SOURCE, "<frozen ast>", Some(ns.clone()), None)?;
    let dict = ns
        .as_dict()
        .ok_or_else(|| PyError::runtime("ast: exec namespace not a dict".into()))?;
    for &name in AST_PY_EXPORTS {
        if let Some(val) = dict.get(&PyKey::str_from(name)) {
            if let ValueKind::PyClass(cls_rc) = val.kind() {
                cls_rc
                    .borrow_mut()
                    .attrs
                    .insert("__module__".to_string(), Value::string("ast"));
            }
            module
                .borrow_mut()
                .attrs
                .insert(name.to_string(), val.clone());
        }
    }
    Ok(Some(ns.clone()))
}
