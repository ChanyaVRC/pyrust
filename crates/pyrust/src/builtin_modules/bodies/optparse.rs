// `optparse` module — 100% native Rust implementation of Python 3.12's
// `optparse` module (issue #3248).
//
// Provides the full CPython 3.12 optparse API: exception hierarchy
// (`OptParseError`, `OptionError`, `OptionConflictError`, `OptionValueError`,
// `BadOptionError`, `AmbiguousOptionError`), constants (`SUPPRESS_HELP`,
// `SUPPRESS_USAGE`, `NO_DEFAULT`, `__version__`), `HelpFormatter`,
// `IndentedHelpFormatter`, `TitledHelpFormatter`, `Option` / `make_option`,
// `Values`, `OptionContainer`, `OptionGroup`, `OptionParser`, `check_builtin`,
// and `check_choice`.
//
// Reference: <https://docs.python.org/3/library/optparse.html>

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::ast::BinaryOp;
use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    FromValue, OptValueRef, OptionalArg, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, is_builtin_callable_adapter, lookup_class_attr,
    primitive_class_by_name, render_value_repr, type_class_singleton, value_class,
    value_type_name_str,
};
use crate::value::{PyDict, PyKey, Value, ValueKind};

fn py_isinstance(interp: &mut Interpreter, obj: &Value, cls: &Value) -> Result<bool> {
    let res = call_pos(
        interp,
        Value::builtin_function("isinstance"),
        &[obj.clone(), cls.clone()],
    )?;
    interp.truthy_value(&res)
}

fn pykey_as_str(k: &PyKey) -> Option<&str> {
    match k {
        PyKey::Str(v) => v.as_str(),
        _ => None,
    }
}

fn dict_remove_str(interp: &mut Interpreter, dict_val: &Value, key: &str) -> Result<()> {
    let pop_fn = interp.get_attr(dict_val, "pop")?;
    let _ = call_pos(interp, pop_fn, &[Value::string(key), Value::none()])?;
    Ok(())
}

fn py_setitem(interp: &mut Interpreter, target: &Value, key: Value, val: Value) -> Result<()> {
    let setitem = interp.get_attr(target, "__setitem__")?;
    call_pos(interp, setitem, &[key, val])?;
    Ok(())
}

fn list_pop_zero(interp: &mut Interpreter, list_val: &Value) -> Result<Value> {
    let pop_fn = interp.get_attr(list_val, "pop")?;
    call_pos(interp, pop_fn, &[Value::int(0)])
}

const SUPPRESS_HELP_STR: &str = "SUPPRESSHELP";
const SUPPRESS_USAGE_STR: &str = "SUPPRESSUSAGE";

const OPTION_ATTRS: [&str; 12] = [
    "action",
    "type",
    "dest",
    "default",
    "nargs",
    "const",
    "choices",
    "callback",
    "callback_args",
    "callback_kwargs",
    "help",
    "metavar",
];

const OPTION_ACTIONS: [&str; 10] = [
    "store",
    "store_const",
    "store_true",
    "store_false",
    "append",
    "append_const",
    "count",
    "callback",
    "help",
    "version",
];

const OPTION_STORE_ACTIONS: [&str; 7] = [
    "store",
    "store_const",
    "store_true",
    "store_false",
    "append",
    "append_const",
    "count",
];

const OPTION_TYPED_ACTIONS: [&str; 3] = ["store", "append", "callback"];
const OPTION_ALWAYS_TYPED_ACTIONS: [&str; 2] = ["store", "append"];
const OPTION_CONST_ACTIONS: [&str; 2] = ["store_const", "append_const"];
const OPTION_TYPES: [&str; 6] = ["string", "int", "long", "float", "complex", "choice"];

fn str_tuple(items: &[&str]) -> Value {
    Value::tuple(items.iter().map(|s| Value::string(*s)).collect())
}

fn str_list(items: &[&str]) -> Value {
    Value::list(items.iter().map(|s| Value::string(*s)).collect())
}

fn make_no_default() -> Value {
    Value::tuple(vec![Value::string("NO"), Value::string("DEFAULT")])
}

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_with_args_kwargs(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &IndexMap<String, Value>,
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some(k.clone()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn optparse_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("optparse")?;
    interp.get_attr(&m, name)
}

fn raise_optparse_exc(interp: &mut Interpreter, exc_name: &str, args: &[Value]) -> PyError {
    match optparse_attr(interp, exc_name).and_then(|cls| call_pos(interp, cls, args)) {
        Ok(exc_inst) => PyError::raised(exc_inst),
        Err(e) => e,
    }
}

fn is_no_default(interp: &mut Interpreter, val: &Value) -> bool {
    if let Ok(nd) = optparse_attr(interp, "NO_DEFAULT")
        && val.object_id() == nd.object_id()
    {
        return true;
    }
    matches!(
        val.kind(),
        ValueKind::Tuple(t)
            if t.len() == 2
                && t[0].as_str() == Some("NO")
                && t[1].as_str() == Some("DEFAULT")
    )
}

fn is_callable_val(v: &Value) -> bool {
    match v.kind() {
        ValueKind::UserFunction(_)
        | ValueKind::BuiltinFunction(_)
        | ValueKind::BoundMethod { .. }
        | ValueKind::ClassBoundMethod { .. }
        | ValueKind::PyClass(_) => true,
        ValueKind::BuiltinObject { .. } => is_builtin_callable_adapter(v),
        ValueKind::PyInstance(inst) => {
            let cls = std::rc::Rc::clone(&inst.borrow().class);
            lookup_class_attr(&cls, "__call__").is_some()
        }
        _ => false,
    }
}

fn repr_optparse_obj(interp: &mut Interpreter, self_val: &Value) -> Result<Value> {
    let cls = value_class(self_val);
    let cls_name = interp
        .get_attr(&cls, "__name__")
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| value_type_name_str(self_val).to_string());
    let obj_id = self_val.object_id().as_int().unwrap_or(0) as u64;
    let s = interp.render_value_as_str(self_val)?;
    Ok(Value::string(format!("<{cls_name} at 0x{obj_id:x}: {s}>")))
}

fn parse_num_int(interp: &mut Interpreter, val: &str) -> Result<Value> {
    let (radix, slice): (i64, &str) = if val.len() >= 2 && val[..2].eq_ignore_ascii_case("0x") {
        (16, val)
    } else if val.len() >= 2 && val[..2].eq_ignore_ascii_case("0b") {
        let rest = &val[2..];
        (2, if rest.is_empty() { "0" } else { rest })
    } else if val.starts_with('0') {
        (8, val)
    } else {
        (10, val)
    };
    let int_cls = primitive_class_by_name("int")
        .map(Value::py_class)
        .unwrap_or_else(|| Value::builtin_function("int"));
    call_pos(interp, int_cls, &[Value::string(slice), Value::int(radix)])
}

fn check_builtin_impl(
    interp: &mut Interpreter,
    option: &Value,
    opt: &str,
    value: &Value,
) -> Result<Value> {
    let type_val = interp.get_attr(option, "type")?;
    let typ = type_val.as_str().unwrap_or("").to_owned();
    let val_str = value.as_str().unwrap_or("").to_owned();
    let (res, what) = match typ.as_str() {
        "int" | "long" => (parse_num_int(interp, &val_str), "integer"),
        "float" => {
            let cls = primitive_class_by_name("float")
                .map(Value::py_class)
                .unwrap_or_else(|| Value::builtin_function("float"));
            (
                call_pos(interp, cls, std::slice::from_ref(value)),
                "floating-point",
            )
        }
        "complex" => {
            let cls = primitive_class_by_name("complex")
                .map(Value::py_class)
                .unwrap_or_else(|| Value::builtin_function("complex"));
            (
                call_pos(interp, cls, std::slice::from_ref(value)),
                "complex",
            )
        }
        _ => {
            return Err(PyError::named("KeyError", typ));
        }
    };
    match res {
        Ok(v) => Ok(v),
        Err(ref e) if e.class_name_is("ValueError") => {
            let vr = render_value_repr(interp, value)?;
            Err(raise_optparse_exc(
                interp,
                "OptionValueError",
                &[Value::string(format!(
                    "option {opt}: invalid {what} value: {vr}"
                ))],
            ))
        }
        Err(e) => Err(e),
    }
}

fn check_choice_impl(
    interp: &mut Interpreter,
    option: &Value,
    opt: &str,
    value: &Value,
) -> Result<Value> {
    let choices_val = interp.get_attr(option, "choices")?;
    let choices = interp.collect_iterable(&choices_val)?;
    for c in &choices {
        let eq = interp.eval_binary(value.clone(), BinaryOp::Eq, c.clone())?;
        if interp.truthy_value(&eq)? {
            return Ok(value.clone());
        }
    }
    let mut rendered = Vec::with_capacity(choices.len());
    for c in &choices {
        rendered.push(render_value_repr(interp, c)?);
    }
    let vr = render_value_repr(interp, value)?;
    Err(raise_optparse_exc(
        interp,
        "OptionValueError",
        &[Value::string(format!(
            "option {opt}: invalid choice: {vr} (choose from {})",
            rendered.join(", ")
        ))],
    ))
}

fn match_abbrev_impl(interp: &mut Interpreter, s: &str, wordmap: &Value) -> Result<Value> {
    let key = PyKey::str_from(s);
    if let Some(dict) = wordmap.as_dict() {
        if dict.contains_key(&key) {
            return Ok(Value::string(s));
        }
        let mut possibilities: Vec<String> = dict
            .keys()
            .filter_map(pykey_as_str)
            .filter(|w| w.starts_with(s))
            .map(str::to_owned)
            .collect();
        if possibilities.len() == 1 {
            return Ok(Value::string(possibilities.pop().unwrap()));
        }
        if possibilities.is_empty() {
            return Err(raise_optparse_exc(
                interp,
                "BadOptionError",
                &[Value::string(s)],
            ));
        }
        possibilities.sort();
        let py_poss = Value::list(possibilities.into_iter().map(Value::string).collect());
        return Err(raise_optparse_exc(
            interp,
            "AmbiguousOptionError",
            &[Value::string(s), py_poss],
        ));
    }
    Err(raise_optparse_exc(
        interp,
        "BadOptionError",
        &[Value::string(s)],
    ))
}

fn instance_dict_value(interp: &mut Interpreter, self_val: &Value) -> Result<Value> {
    interp.get_attr(self_val, "__dict__")
}

fn seq_contains(interp: &mut Interpreter, seq: &Value, item: &Value) -> Result<bool> {
    let elems = interp.collect_iterable(seq)?;
    for e in elems {
        let eq = interp.eval_binary(item.clone(), BinaryOp::Eq, e)?;
        if interp.truthy_value(&eq)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn list_remove_first(interp: &mut Interpreter, list_val: &Value, target: &Value) -> Result<()> {
    let remove_fn = interp.get_attr(list_val, "remove")?;
    call_pos(interp, remove_fn, std::slice::from_ref(target))?;
    Ok(())
}

fn dict_get_str(dict_val: &Value, k: &str) -> Option<Value> {
    dict_val
        .as_dict()
        .and_then(|d| d.get(&PyKey::str_from(k)).cloned())
}

fn dict_contains_str(dict_val: &Value, k: &str) -> bool {
    dict_val
        .as_dict()
        .is_some_and(|d| d.contains_key(&PyKey::str_from(k)))
}

fn attr_i64(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<i64> {
    let v = interp.get_attr(obj, name)?;
    match v.as_int() {
        Some(n) => Ok(n),
        None => PyIndex::try_from_value_with_interp(interp, &v, name, name)?.expect_i64(name, name),
    }
}

fn attr_str_opt(interp: &mut Interpreter, obj: &Value, name: &str) -> Result<Option<String>> {
    let v = interp.get_attr(obj, name)?;
    if v.is_none() {
        return Ok(None);
    }
    Ok(v.as_str().map(str::to_owned))
}

fn list_of_strings(interp: &mut Interpreter, list_val: &Value) -> Result<Vec<String>> {
    let items = interp.collect_iterable(list_val)?;
    Ok(items
        .into_iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect())
}

fn textwrap_fill(interp: &mut Interpreter, text: &str, width: i64, indent: &str) -> Result<String> {
    let tw = interp.load_module("textwrap")?;
    let fill_fn = interp.get_attr(&tw, "fill")?;
    let res = interp.call_function_expanded(
        fill_fn,
        &[
            ExpandedCallArg {
                name: None,
                value: Value::string(text),
            },
            ExpandedCallArg {
                name: None,
                value: Value::int(width),
            },
            ExpandedCallArg {
                name: Some("initial_indent".to_string()),
                value: Value::string(indent),
            },
            ExpandedCallArg {
                name: Some("subsequent_indent".to_string()),
                value: Value::string(indent),
            },
        ],
    )?;
    Ok(res.as_str().unwrap_or("").to_owned())
}

fn textwrap_wrap(interp: &mut Interpreter, text: &str, width: i64) -> Result<Vec<String>> {
    let tw = interp.load_module("textwrap")?;
    let wrap_fn = interp.get_attr(&tw, "wrap")?;
    let res = call_pos(interp, wrap_fn, &[Value::string(text), Value::int(width)])?;
    list_of_strings(interp, &res)
}

fn write_to_file_or_stdout(
    interp: &mut Interpreter,
    file: Option<&Value>,
    text: &str,
) -> Result<()> {
    let target = match file.filter(|v| !v.is_none()) {
        Some(f) => f.clone(),
        None => {
            let sys_mod = interp.load_module("sys")?;
            interp.get_attr(&sys_mod, "stdout")?
        }
    };
    let write_fn = interp.get_attr(&target, "write")?;
    call_pos(interp, write_fn, &[Value::string(text)])?;
    Ok(())
}

/// Wire `make_option = Option`, `Option.TYPE_CHECKER`, and `Option.CHECK_METHODS`
/// on a freshly loaded `optparse` module.
pub(crate) fn prepare_module(_interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    let option_cls = mod_rc.borrow().attrs.get("Option").cloned();
    let check_builtin_fn = mod_rc.borrow().attrs.get("check_builtin").cloned();
    let check_choice_fn = mod_rc.borrow().attrs.get("check_choice").cloned();

    if let Some(opt_cls) = option_cls {
        mod_rc
            .borrow_mut()
            .insert_attr("make_option".to_string(), opt_cls.clone());
        if let ValueKind::PyClass(cls_rc) = opt_cls.kind() {
            let mut tc = PyDict::default();
            if let Some(cb) = check_builtin_fn {
                for k in ["int", "long", "float", "complex"] {
                    tc.insert(PyKey::str_from(k), cb.clone());
                }
            }
            if let Some(cc) = check_choice_fn {
                tc.insert(PyKey::str_from("choice"), cc);
            }
            let check_methods: Vec<Value> = [
                "_check_action",
                "_check_type",
                "_check_choice",
                "_check_dest",
                "_check_const",
                "_check_nargs",
                "_check_callback",
            ]
            .iter()
            .filter_map(|m| cls_rc.borrow().attrs.get(*m).cloned())
            .collect();
            let mut cls = cls_rc.borrow_mut();
            cls.attrs
                .insert("TYPE_CHECKER".to_string(), Value::dict(tc));
            cls.attrs
                .insert("CHECK_METHODS".to_string(), Value::list(check_methods));
        }
    }
}

pyrust_module! {
    constants {
        "__version__" => Value::string("1.5.3"),
        "__all__" => str_list(&[
            "Option",
            "make_option",
            "SUPPRESS_HELP",
            "SUPPRESS_USAGE",
            "Values",
            "OptionContainer",
            "OptionGroup",
            "OptionParser",
            "HelpFormatter",
            "IndentedHelpFormatter",
            "TitledHelpFormatter",
            "OptParseError",
            "OptionError",
            "OptionConflictError",
            "OptionValueError",
            "BadOptionError",
            "check_choice",
        ]),
        "SUPPRESS_HELP" => Value::string(SUPPRESS_HELP_STR),
        "SUPPRESS_USAGE" => Value::string(SUPPRESS_USAGE_STR),
        "NO_DEFAULT" => make_no_default(),
    }

    // ── Module-level helper functions ────────────────────────────────────────

    fn check_builtin(option: PyValueRef<'_>, opt: PyStr<'_>, value: PyValueRef<'_>) -> Result<Value> {
        check_builtin_impl(_interp, option.0, &opt, value.0)
    }

    fn check_choice(option: PyValueRef<'_>, opt: PyStr<'_>, value: PyValueRef<'_>) -> Result<Value> {
        check_choice_impl(_interp, option.0, &opt, value.0)
    }

    fn _match_abbrev(s: PyStr<'_>, wordmap: PyValueRef<'_>) -> Result<Value> {
        match_abbrev_impl(_interp, &s, wordmap.0)
    }

    // ── Exception hierarchy ──────────────────────────────────────────────────

    class OptParseError("Exception") {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>, msg: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "msg", msg.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "args", Value::tuple(vec![msg.0]))?;
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "msg")
        }
    }

    class OptionError("OptParseError") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            msg: PyValue,
            option: PyValue,
        ) -> Result<Value> {
            let opt_id = _interp.render_value_as_str(&option.0)?;
            _interp.assign_attr(self_val.0.clone(), "msg", msg.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "option_id", Value::string(opt_id))?;
            _interp.assign_attr(self_val.0.clone(), "args", Value::tuple(vec![msg.0, option.0]))?;
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let opt_id = _interp.get_attr(self_val.0, "option_id")?;
            let msg = _interp.get_attr(self_val.0, "msg")?;
            let msg_s = _interp.render_value_as_str(&msg)?;
            if _interp.truthy_value(&opt_id)? {
                let id_s = _interp.render_value_as_str(&opt_id)?;
                Ok(Value::string(format!("option {id_s}: {msg_s}")))
            } else {
                Ok(Value::string(msg_s))
            }
        }
    }

    /// Raised if conflicting options are added to an OptionParser.
    exception OptionConflictError(OptionError);

    /// Raised if an invalid option value is encountered on the command line.
    exception OptionValueError(OptParseError);

    class BadOptionError("OptParseError") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            opt_str: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "opt_str", opt_str.0.clone())?;
            _interp.assign_attr(self_val.0.clone(), "args", Value::tuple(vec![opt_str.0]))?;
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let opt_str = _interp.get_attr(self_val.0, "opt_str")?;
            let s = _interp.render_value_as_str(&opt_str)?;
            Ok(Value::string(format!("no such option: {s}")))
        }
    }

    class AmbiguousOptionError("BadOptionError") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            opt_str: PyValue,
            possibilities: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "opt_str", opt_str.0.clone())?;
            _interp.assign_attr(
                self_val.0.clone(),
                "possibilities",
                possibilities.0.clone(),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "args",
                Value::tuple(vec![opt_str.0, possibilities.0]),
            )?;
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let opt_str = _interp.get_attr(self_val.0, "opt_str")?;
            let poss = _interp.get_attr(self_val.0, "possibilities")?;
            let opt_s = _interp.render_value_as_str(&opt_str)?;
            let parts = list_of_strings(_interp, &poss)?;
            Ok(Value::string(format!(
                "ambiguous option: {opt_s} ({}?)",
                parts.join(", ")
            )))
        }
    }

    // ── HelpFormatter hierarchy ──────────────────────────────────────────────

    class HelpFormatter {
        constants {
            "NO_DEFAULT_VALUE" => Value::string("none"),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            indent_increment: PyIndex,
            max_help_position: PyIndex,
            width: PyValue,
            short_first: PyValue,
        ) -> Result<Value> {
            let inc = indent_increment.expect_i64(FN_NAME, "indent_increment")?;
            let max_hp = max_help_position.expect_i64(FN_NAME, "max_help_position")?;
            let w: i64 = if width.0.is_none() {
                let env_w = std::env::var("COLUMNS")
                    .ok()
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(80);
                env_w - 2
            } else {
                PyIndex::try_from_value_with_interp(_interp, &width.0, FN_NAME, "width")?
                    .expect_i64(FN_NAME, "width")?
            };
            let hp = max_hp.min((w - 20).max(inc * 2));
            _interp.assign_attr(self_val.0.clone(), "parser", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "indent_increment", Value::int(inc))?;
            _interp.assign_attr(self_val.0.clone(), "width", Value::int(w))?;
            _interp.assign_attr(self_val.0.clone(), "max_help_position", Value::int(hp))?;
            _interp.assign_attr(self_val.0.clone(), "help_position", Value::int(hp))?;
            _interp.assign_attr(self_val.0.clone(), "current_indent", Value::int(0))?;
            _interp.assign_attr(self_val.0.clone(), "level", Value::int(0))?;
            _interp.assign_attr(self_val.0.clone(), "help_width", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "short_first", short_first.0)?;
            _interp.assign_attr(
                self_val.0.clone(),
                "default_tag",
                Value::string("%default"),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "option_strings",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(self_val.0.clone(), "_short_opt_fmt", Value::string("%s %s"))?;
            _interp.assign_attr(self_val.0.clone(), "_long_opt_fmt", Value::string("%s=%s"))?;
            Ok(Value::none())
        }

        fn set_parser(#[positional_only] self_val: PyValueRef<'_>, parser: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "parser", parser.0)?;
            Ok(Value::none())
        }

        fn set_short_opt_delimiter(
            #[positional_only] self_val: PyValueRef<'_>,
            delim: PyStr<'_>,
        ) -> Result<Value> {
            if !delim.is_empty() && &*delim != " " {
                let r = render_value_repr(_interp, &Value::string(&*delim))?;
                return Err(PyError::named(
                    "ValueError",
                    format!("invalid metavar delimiter for short options: {r}"),
                ));
            }
            _interp.assign_attr(
                self_val.0.clone(),
                "_short_opt_fmt",
                Value::string(format!("%s{}%s", &*delim)),
            )?;
            Ok(Value::none())
        }

        fn set_long_opt_delimiter(
            #[positional_only] self_val: PyValueRef<'_>,
            delim: PyStr<'_>,
        ) -> Result<Value> {
            if &*delim != "=" && &*delim != " " {
                let r = render_value_repr(_interp, &Value::string(&*delim))?;
                return Err(PyError::named(
                    "ValueError",
                    format!("invalid metavar delimiter for long options: {r}"),
                ));
            }
            _interp.assign_attr(
                self_val.0.clone(),
                "_long_opt_fmt",
                Value::string(format!("%s{}%s", &*delim)),
            )?;
            Ok(Value::none())
        }

        fn indent(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cur = attr_i64(_interp, self_val.0, "current_indent")?;
            let inc = attr_i64(_interp, self_val.0, "indent_increment")?;
            let lvl = attr_i64(_interp, self_val.0, "level")?;
            _interp.assign_attr(self_val.0.clone(), "current_indent", Value::int(cur + inc))?;
            _interp.assign_attr(self_val.0.clone(), "level", Value::int(lvl + 1))?;
            Ok(Value::none())
        }

        fn dedent(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cur = attr_i64(_interp, self_val.0, "current_indent")?;
            let inc = attr_i64(_interp, self_val.0, "indent_increment")?;
            let lvl = attr_i64(_interp, self_val.0, "level")?;
            if cur - inc < 0 {
                return Err(PyError::named("AssertionError", "Indent decreased below 0."));
            }
            _interp.assign_attr(self_val.0.clone(), "current_indent", Value::int(cur - inc))?;
            _interp.assign_attr(self_val.0.clone(), "level", Value::int(lvl - 1))?;
            Ok(Value::none())
        }

        fn format_usage(
            #[positional_only] _self_val: PyValueRef<'_>,
            _usage: PyValue,
        ) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "subclasses must implement"))
        }

        fn format_heading(
            #[positional_only] _self_val: PyValueRef<'_>,
            _heading: PyValue,
        ) -> Result<Value> {
            Err(PyError::named("NotImplementedError", "subclasses must implement"))
        }

        fn _format_text(#[positional_only] self_val: PyValueRef<'_>, text: PyStr<'_>) -> Result<Value> {
            let width = attr_i64(_interp, self_val.0, "width")?;
            let cur = attr_i64(_interp, self_val.0, "current_indent")?;
            let text_width = (width - cur).max(11);
            let indent = " ".repeat(cur.max(0) as usize);
            let filled = textwrap_fill(_interp, &text, text_width, &indent)?;
            Ok(Value::string(filled))
        }

        fn format_description(
            #[positional_only] self_val: PyValueRef<'_>,
            description: PyValue,
        ) -> Result<Value> {
            if _interp.truthy_value(&description.0)? {
                let fmt_text = _interp.get_attr(self_val.0, "_format_text")?;
                let res = call_pos(_interp, fmt_text, &[description.0])?;
                let s = res.as_str().unwrap_or("");
                Ok(Value::string(format!("{s}\n")))
            } else {
                Ok(Value::string(""))
            }
        }

        fn format_epilog(
            #[positional_only] self_val: PyValueRef<'_>,
            epilog: PyValue,
        ) -> Result<Value> {
            if _interp.truthy_value(&epilog.0)? {
                let fmt_text = _interp.get_attr(self_val.0, "_format_text")?;
                let res = call_pos(_interp, fmt_text, &[epilog.0])?;
                let s = res.as_str().unwrap_or("");
                Ok(Value::string(format!("\n{s}\n")))
            } else {
                Ok(Value::string(""))
            }
        }

        fn expand_default(
            #[positional_only] self_val: PyValueRef<'_>,
            option: PyValue,
        ) -> Result<Value> {
            let parser = _interp.get_attr(self_val.0, "parser")?;
            let default_tag = _interp.get_attr(self_val.0, "default_tag")?;
            let help = _interp.get_attr(&option.0, "help")?;
            if parser.is_none() || !_interp.truthy_value(&default_tag)? {
                return Ok(help);
            }
            let defaults = _interp.get_attr(&parser, "defaults")?;
            let dest = _interp.get_attr(&option.0, "dest")?;
            let default_val = dest
                .as_str()
                .and_then(|k| dict_get_str(&defaults, k))
                .unwrap_or_else(Value::none);
            let resolved = if is_no_default(_interp, &default_val) || default_val.is_none() {
                _interp.get_attr(self_val.0, "NO_DEFAULT_VALUE")?
            } else {
                default_val
            };
            let help_s = help.as_str().unwrap_or("");
            let tag_s = default_tag.as_str().unwrap_or("");
            let val_s = _interp.render_value_as_str(&resolved)?;
            Ok(Value::string(help_s.replace(tag_s, &val_s)))
        }

        fn format_option(
            #[positional_only] self_val: PyValueRef<'_>,
            option: PyValue,
        ) -> Result<Value> {
            let opt_strings = _interp.get_attr(self_val.0, "option_strings")?;
            let opts_val = _interp.eval_index(&opt_strings, option.0.clone())?;
            let opts = opts_val.as_str().unwrap_or("").to_owned();
            let help_pos = attr_i64(_interp, self_val.0, "help_position")?.max(0) as usize;
            let cur_indent = attr_i64(_interp, self_val.0, "current_indent")?.max(0) as usize;
            let opt_width = help_pos.saturating_sub(cur_indent + 2);
            let mut result = String::new();
            let indent_first: usize = if opts.chars().count() > opt_width {
                result.push_str(&" ".repeat(cur_indent));
                result.push_str(&opts);
                result.push('\n');
                help_pos
            } else {
                result.push_str(&" ".repeat(cur_indent));
                result.push_str(&opts);
                let pad = opt_width.saturating_sub(opts.chars().count());
                result.push_str(&" ".repeat(pad + 2));
                0
            };
            let help = _interp.get_attr(&option.0, "help")?;
            if _interp.truthy_value(&help)? {
                let expand_fn = _interp.get_attr(self_val.0, "expand_default")?;
                let help_text_val = call_pos(_interp, expand_fn, &[option.0])?;
                let help_text = help_text_val.as_str().unwrap_or("");
                let help_width = attr_i64(_interp, self_val.0, "help_width")?;
                let lines = textwrap_wrap(_interp, help_text, help_width)?;
                if let Some((first, rest)) = lines.split_first() {
                    result.push_str(&" ".repeat(indent_first));
                    result.push_str(first);
                    result.push('\n');
                    for line in rest {
                        result.push_str(&" ".repeat(help_pos));
                        result.push_str(line);
                        result.push('\n');
                    }
                }
            } else if !result.ends_with('\n') {
                result.push('\n');
            }
            Ok(Value::string(result))
        }

        fn store_option_strings(
            #[positional_only] self_val: PyValueRef<'_>,
            parser: PyValue,
        ) -> Result<Value> {
            let indent_fn = _interp.get_attr(self_val.0, "indent")?;
            let dedent_fn = _interp.get_attr(self_val.0, "dedent")?;
            let fmt_opts_fn = _interp.get_attr(self_val.0, "format_option_strings")?;
            let opt_strings = _interp.get_attr(self_val.0, "option_strings")?;

            call_pos(_interp, indent_fn.clone(), &[])?;
            let mut max_len: i64 = 0;
            let opt_list_val = _interp.get_attr(&parser.0, "option_list")?;
            let opt_list = _interp.collect_iterable(&opt_list_val)?;
            let cur1 = attr_i64(_interp, self_val.0, "current_indent")?;
            for opt in opt_list {
                let strings = call_pos(_interp, fmt_opts_fn.clone(), std::slice::from_ref(&opt))?;
                let slen = strings.as_str().map_or(0, |s| s.chars().count()) as i64;
                py_setitem(_interp, &opt_strings, opt, strings)?;
                max_len = max_len.max(slen + cur1);
            }
            call_pos(_interp, indent_fn, &[])?;
            let cur2 = attr_i64(_interp, self_val.0, "current_indent")?;
            let groups_val = _interp.get_attr(&parser.0, "option_groups")?;
            let groups = _interp.collect_iterable(&groups_val)?;
            for grp in groups {
                let g_opts_val = _interp.get_attr(&grp, "option_list")?;
                let g_opts = _interp.collect_iterable(&g_opts_val)?;
                for opt in g_opts {
                    let strings =
                        call_pos(_interp, fmt_opts_fn.clone(), std::slice::from_ref(&opt))?;
                    let slen = strings.as_str().map_or(0, |s| s.chars().count()) as i64;
                    py_setitem(_interp, &opt_strings, opt, strings)?;
                    max_len = max_len.max(slen + cur2);
                }
            }
            call_pos(_interp, dedent_fn.clone(), &[])?;
            call_pos(_interp, dedent_fn, &[])?;

            let max_hp = attr_i64(_interp, self_val.0, "max_help_position")?;
            let width = attr_i64(_interp, self_val.0, "width")?;
            let help_pos = (max_len + 2).min(max_hp);
            let help_width = (width - help_pos).max(11);
            _interp.assign_attr(self_val.0.clone(), "help_position", Value::int(help_pos))?;
            _interp.assign_attr(self_val.0.clone(), "help_width", Value::int(help_width))?;
            Ok(Value::none())
        }

        fn format_option_strings(
            #[positional_only] self_val: PyValueRef<'_>,
            option: PyValue,
        ) -> Result<Value> {
            let takes_val_fn = _interp.get_attr(&option.0, "takes_value")?;
            let takes_res = call_pos(_interp, takes_val_fn, &[])?;
            let takes_val = _interp.truthy_value(&takes_res)?;
            let short_attr = _interp.get_attr(&option.0, "_short_opts")?;
            let short_raw = list_of_strings(_interp, &short_attr)?;
            let long_attr = _interp.get_attr(&option.0, "_long_opts")?;
            let long_raw = list_of_strings(_interp, &long_attr)?;

            let (short_opts, long_opts): (Vec<String>, Vec<String>) = if takes_val {
                let metavar = match attr_str_opt(_interp, &option.0, "metavar")? {
                    Some(m) if !m.is_empty() => m,
                    _ => attr_str_opt(_interp, &option.0, "dest")?
                        .unwrap_or_default()
                        .to_uppercase(),
                };
                let sfmt = attr_str_opt(_interp, self_val.0, "_short_opt_fmt")?
                    .unwrap_or_else(|| "%s %s".to_string());
                let lfmt = attr_str_opt(_interp, self_val.0, "_long_opt_fmt")?
                    .unwrap_or_else(|| "%s=%s".to_string());
                let s_delim = sfmt.strip_prefix("%s").and_then(|r| r.strip_suffix("%s")).unwrap_or(" ");
                let l_delim = lfmt.strip_prefix("%s").and_then(|r| r.strip_suffix("%s")).unwrap_or("=");
                let so = short_raw
                    .into_iter()
                    .map(|s| format!("{s}{s_delim}{metavar}"))
                    .collect();
                let lo = long_raw
                    .into_iter()
                    .map(|l| format!("{l}{l_delim}{metavar}"))
                    .collect();
                (so, lo)
            } else {
                (short_raw, long_raw)
            };

            let sf_val = _interp.get_attr(self_val.0, "short_first")?;
            let short_first = _interp.truthy_value(&sf_val)?;
            let mut combined = Vec::with_capacity(short_opts.len() + long_opts.len());
            if short_first {
                combined.extend(short_opts);
                combined.extend(long_opts);
            } else {
                combined.extend(long_opts);
                combined.extend(short_opts);
            }
            Ok(Value::string(combined.join(", ")))
        }
    }

    class IndentedHelpFormatter("HelpFormatter") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] indent_increment: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] max_help_position: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] width: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] short_first: OptValueRef<'_>,
        ) -> Result<Value> {
            let hf_cls = optparse_attr(_interp, "HelpFormatter")?;
            let init_fn = _interp.get_attr(&hf_cls, "__init__")?;
            call_pos(
                _interp,
                init_fn,
                &[
                    self_val.0.clone(),
                    indent_increment
                        .as_value()
                        .cloned()
                        .unwrap_or_else(|| Value::int(2)),
                    max_help_position
                        .as_value()
                        .cloned()
                        .unwrap_or_else(|| Value::int(24)),
                    width.as_value().cloned().unwrap_or_else(Value::none),
                    short_first
                        .as_value()
                        .cloned()
                        .unwrap_or_else(|| Value::int(1)),
                ],
            )
        }

        fn format_usage(
            #[positional_only] _self_val: PyValueRef<'_>,
            usage: PyStr<'_>,
        ) -> Result<Value> {
            Ok(Value::string(format!("Usage: {}\n", &*usage)))
        }

        fn format_heading(
            #[positional_only] self_val: PyValueRef<'_>,
            heading: PyStr<'_>,
        ) -> Result<Value> {
            let cur = attr_i64(_interp, self_val.0, "current_indent")?.max(0) as usize;
            Ok(Value::string(format!("{}{}:\n", " ".repeat(cur), &*heading)))
        }
    }

    class TitledHelpFormatter("HelpFormatter") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] indent_increment: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] max_help_position: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] width: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] short_first: OptValueRef<'_>,
        ) -> Result<Value> {
            let hf_cls = optparse_attr(_interp, "HelpFormatter")?;
            let init_fn = _interp.get_attr(&hf_cls, "__init__")?;
            call_pos(
                _interp,
                init_fn,
                &[
                    self_val.0.clone(),
                    indent_increment
                        .as_value()
                        .cloned()
                        .unwrap_or_else(|| Value::int(0)),
                    max_help_position
                        .as_value()
                        .cloned()
                        .unwrap_or_else(|| Value::int(24)),
                    width.as_value().cloned().unwrap_or_else(Value::none),
                    short_first
                        .as_value()
                        .cloned()
                        .unwrap_or_else(|| Value::int(0)),
                ],
            )
        }

        fn format_usage(
            #[positional_only] self_val: PyValueRef<'_>,
            usage: PyStr<'_>,
        ) -> Result<Value> {
            let fh = _interp.get_attr(self_val.0, "format_heading")?;
            let heading = call_pos(_interp, fh, &[Value::string("Usage")])?;
            let h = heading.as_str().unwrap_or("");
            Ok(Value::string(format!("{h}  {}\n", &*usage)))
        }

        fn format_heading(
            #[positional_only] self_val: PyValueRef<'_>,
            heading: PyStr<'_>,
        ) -> Result<Value> {
            let lvl = attr_i64(_interp, self_val.0, "level")?;
            let ch = if lvl == 0 { '=' } else { '-' };
            let underline: String = std::iter::repeat_n(ch, heading.chars().count()).collect();
            Ok(Value::string(format!("{}\n{underline}\n", &*heading)))
        }
    }

    // ── Option ───────────────────────────────────────────────────────────────

    class Option {
        constants {
            "ATTRS" => str_list(&OPTION_ATTRS),
            "ACTIONS" => str_tuple(&OPTION_ACTIONS),
            "STORE_ACTIONS" => str_tuple(&OPTION_STORE_ACTIONS),
            "TYPED_ACTIONS" => str_tuple(&OPTION_TYPED_ACTIONS),
            "ALWAYS_TYPED_ACTIONS" => str_tuple(&OPTION_ALWAYS_TYPED_ACTIONS),
            "CONST_ACTIONS" => str_tuple(&OPTION_CONST_ACTIONS),
            "TYPES" => str_tuple(&OPTION_TYPES),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] opts: &[Value],
            #[varkwargs] attrs: IndexMap<String, Value>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_short_opts", Value::list(Vec::new()))?;
            _interp.assign_attr(self_val.0.clone(), "_long_opts", Value::list(Vec::new()))?;

            let check_opts = _interp.get_attr(self_val.0, "_check_opt_strings")?;
            let checked = call_pos(_interp, check_opts, &[Value::tuple(opts.to_vec())])?;
            let set_opts = _interp.get_attr(self_val.0, "_set_opt_strings")?;
            call_pos(_interp, set_opts, &[checked])?;

            let mut kw_dict = PyDict::default();
            for (k, v) in attrs {
                kw_dict.insert(PyKey::str_from(k), v);
            }
            let set_attrs = _interp.get_attr(self_val.0, "_set_attrs")?;
            call_pos(_interp, set_attrs, &[Value::dict(kw_dict)])?;

            let check_methods = _interp.get_attr(self_val.0, "CHECK_METHODS")?;
            let checkers = _interp.collect_iterable(&check_methods)?;
            for checker in checkers {
                call_pos(_interp, checker, std::slice::from_ref(self_val.0))?;
            }
            Ok(Value::none())
        }

        fn _check_opt_strings(
            #[positional_only] _self_val: PyValueRef<'_>,
            opts: PyValue,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(&opts.0)?;
            let mut filtered = Vec::new();
            for item in items {
                if _interp.truthy_value(&item)? {
                    filtered.push(item);
                }
            }
            if filtered.is_empty() {
                return Err(PyError::named(
                    "TypeError",
                    "at least one option string must be supplied",
                ));
            }
            Ok(Value::list(filtered))
        }

        fn _set_opt_strings(
            #[positional_only] self_val: PyValueRef<'_>,
            opts: PyValue,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(&opts.0)?;
            let short_list = _interp.get_attr(self_val.0, "_short_opts")?;
            let long_list = _interp.get_attr(self_val.0, "_long_opts")?;
            for item in items {
                let Some(opt) = item.as_str() else {
                    return Err(PyError::named("TypeError", "option string must be str"));
                };
                let chars: Vec<char> = opt.chars().collect();
                if chars.len() < 2 {
                    let r = render_value_repr(_interp, &item)?;
                    return Err(raise_optparse_exc(
                        _interp,
                        "OptionError",
                        &[
                            Value::string(format!(
                                "invalid option string {r}: must be at least two characters long"
                            )),
                            self_val.0.clone(),
                        ],
                    ));
                } else if chars.len() == 2 {
                    if !(chars[0] == '-' && chars[1] != '-') {
                        let r = render_value_repr(_interp, &item)?;
                        return Err(raise_optparse_exc(
                            _interp,
                            "OptionError",
                            &[
                                Value::string(format!(
                                    "invalid short option string {r}: must be of the form -x, (x any non-dash char)"
                                )),
                                self_val.0.clone(),
                            ],
                        ));
                    }
                    let app = _interp.get_attr(&short_list, "append")?;
                    call_pos(_interp, app, &[item])?;
                } else {
                    if !(chars[0] == '-' && chars[1] == '-' && chars[2] != '-') {
                        let r = render_value_repr(_interp, &item)?;
                        return Err(raise_optparse_exc(
                            _interp,
                            "OptionError",
                            &[
                                Value::string(format!(
                                    "invalid long option string {r}: must start with --, followed by non-dash"
                                )),
                                self_val.0.clone(),
                            ],
                        ));
                    }
                    let app = _interp.get_attr(&long_list, "append")?;
                    call_pos(_interp, app, &[item])?;
                }
            }
            Ok(Value::none())
        }

        fn _set_attrs(#[positional_only] self_val: PyValueRef<'_>, attrs: PyValue) -> Result<Value> {
            let attr_names_val = _interp.get_attr(self_val.0, "ATTRS")?;
            let attr_names = list_of_strings(_interp, &attr_names_val)?;
            let no_default = optparse_attr(_interp, "NO_DEFAULT")?;
            for attr in &attr_names {
                let key = PyKey::str_from(attr);
                let found = attrs.0.as_dict().and_then(|d| d.get(&key).cloned());
                if let Some(val) = found {
                    _interp.assign_attr(self_val.0.clone(), attr, val)?;
                    dict_remove_str(_interp, &attrs.0, attr)?;
                } else if attr == "default" {
                    _interp.assign_attr(self_val.0.clone(), attr, no_default.clone())?;
                } else {
                    _interp.assign_attr(self_val.0.clone(), attr, Value::none())?;
                }
            }
            let leftover: Vec<String> = attrs
                .0
                .as_dict()
                .map(|d| {
                    let mut ks: Vec<String> = d
                        .keys()
                        .filter_map(|k| pykey_as_str(k).map(str::to_owned))
                        .collect();
                    ks.sort();
                    ks
                })
                .unwrap_or_default();
            if !leftover.is_empty() {
                return Err(raise_optparse_exc(
                    _interp,
                    "OptionError",
                    &[
                        Value::string(format!("invalid keyword arguments: {}", leftover.join(", "))),
                        self_val.0.clone(),
                    ],
                ));
            }
            Ok(Value::none())
        }

        fn _check_action(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let action = _interp.get_attr(self_val.0, "action")?;
            if action.is_none() {
                _interp.assign_attr(self_val.0.clone(), "action", Value::string("store"))?;
            } else {
                let actions = _interp.get_attr(self_val.0, "ACTIONS")?;
                if !seq_contains(_interp, &actions, &action)? {
                    let r = render_value_repr(_interp, &action)?;
                    return Err(raise_optparse_exc(
                        _interp,
                        "OptionError",
                        &[
                            Value::string(format!("invalid action: {r}")),
                            self_val.0.clone(),
                        ],
                    ));
                }
            }
            Ok(Value::none())
        }

        fn _check_type(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let mut typ = _interp.get_attr(self_val.0, "type")?;
            let action = _interp.get_attr(self_val.0, "action")?;
            if typ.is_none() {
                let always_typed = _interp.get_attr(self_val.0, "ALWAYS_TYPED_ACTIONS")?;
                if seq_contains(_interp, &always_typed, &action)? {
                    let choices = _interp.get_attr(self_val.0, "choices")?;
                    if !choices.is_none() {
                        _interp.assign_attr(self_val.0.clone(), "type", Value::string("choice"))?;
                    } else {
                        _interp.assign_attr(self_val.0.clone(), "type", Value::string("string"))?;
                    }
                }
            } else {
                let type_cls = Value::py_class(type_class_singleton());
                if py_isinstance(_interp, &typ, &type_cls)? {
                    typ = _interp.get_attr(&typ, "__name__")?;
                    _interp.assign_attr(self_val.0.clone(), "type", typ.clone())?;
                }
                if typ.as_str() == Some("str") {
                    typ = Value::string("string");
                    _interp.assign_attr(self_val.0.clone(), "type", typ.clone())?;
                }
                let types = _interp.get_attr(self_val.0, "TYPES")?;
                if !seq_contains(_interp, &types, &typ)? {
                    let r = render_value_repr(_interp, &typ)?;
                    return Err(raise_optparse_exc(
                        _interp,
                        "OptionError",
                        &[
                            Value::string(format!("invalid option type: {r}")),
                            self_val.0.clone(),
                        ],
                    ));
                }
                let typed_actions = _interp.get_attr(self_val.0, "TYPED_ACTIONS")?;
                if !seq_contains(_interp, &typed_actions, &action)? {
                    let r = render_value_repr(_interp, &action)?;
                    return Err(raise_optparse_exc(
                        _interp,
                        "OptionError",
                        &[
                            Value::string(format!("must not supply a type for action {r}")),
                            self_val.0.clone(),
                        ],
                    ));
                }
            }
            Ok(Value::none())
        }

        fn _check_choice(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let typ = _interp.get_attr(self_val.0, "type")?;
            let choices = _interp.get_attr(self_val.0, "choices")?;
            if typ.as_str() == Some("choice") {
                if choices.is_none() {
                    return Err(raise_optparse_exc(
                        _interp,
                        "OptionError",
                        &[
                            Value::string("must supply a list of choices for type 'choice'"),
                            self_val.0.clone(),
                        ],
                    ));
                }
                let tup_cls = primitive_class_by_name("tuple").map(Value::py_class);
                let list_cls = primitive_class_by_name("list").map(Value::py_class);
                let is_seq = match (tup_cls, list_cls) {
                    (Some(tc), Some(lc)) => {
                        py_isinstance(_interp, &choices, &Value::tuple(vec![tc, lc]))?
                    }
                    _ => matches!(choices.kind(), ValueKind::Tuple(_) | ValueKind::List(_)),
                };
                if !is_seq {
                    let tname = value_type_name_str(&choices);
                    return Err(raise_optparse_exc(
                        _interp,
                        "OptionError",
                        &[
                            Value::string(format!(
                                "choices must be a list of strings ('{tname}' supplied)"
                            )),
                            self_val.0.clone(),
                        ],
                    ));
                }
            } else if !choices.is_none() {
                let r = render_value_repr(_interp, &typ)?;
                return Err(raise_optparse_exc(
                    _interp,
                    "OptionError",
                    &[
                        Value::string(format!("must not supply choices for type {r}")),
                        self_val.0.clone(),
                    ],
                ));
            }
            Ok(Value::none())
        }

        fn _check_dest(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let action = _interp.get_attr(self_val.0, "action")?;
            let store_actions = _interp.get_attr(self_val.0, "STORE_ACTIONS")?;
            let typ = _interp.get_attr(self_val.0, "type")?;
            let takes_value =
                seq_contains(_interp, &store_actions, &action)? || !typ.is_none();
            let dest = _interp.get_attr(self_val.0, "dest")?;
            if dest.is_none() && takes_value {
                let lo_attr = _interp.get_attr(self_val.0, "_long_opts")?;
                let long_opts = list_of_strings(_interp, &lo_attr)?;
                let derived = if let Some(first_long) = long_opts.first() {
                    first_long[2..].replace('-', "_")
                } else {
                    let so_attr = _interp.get_attr(self_val.0, "_short_opts")?;
                    let short_opts = list_of_strings(_interp, &so_attr)?;
                    short_opts
                        .first()
                        .and_then(|s| s.chars().nth(1))
                        .map(|c| c.to_string())
                        .unwrap_or_default()
                };
                _interp.assign_attr(self_val.0.clone(), "dest", Value::string(derived))?;
            }
            Ok(Value::none())
        }

        fn _check_const(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let action = _interp.get_attr(self_val.0, "action")?;
            let const_actions = _interp.get_attr(self_val.0, "CONST_ACTIONS")?;
            let const_val = _interp.get_attr(self_val.0, "const")?;
            if !seq_contains(_interp, &const_actions, &action)? && !const_val.is_none() {
                let r = render_value_repr(_interp, &action)?;
                return Err(raise_optparse_exc(
                    _interp,
                    "OptionError",
                    &[
                        Value::string(format!("'const' must not be supplied for action {r}")),
                        self_val.0.clone(),
                    ],
                ));
            }
            Ok(Value::none())
        }

        fn _check_nargs(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let action = _interp.get_attr(self_val.0, "action")?;
            let typed_actions = _interp.get_attr(self_val.0, "TYPED_ACTIONS")?;
            let nargs = _interp.get_attr(self_val.0, "nargs")?;
            if seq_contains(_interp, &typed_actions, &action)? {
                if nargs.is_none() {
                    _interp.assign_attr(self_val.0.clone(), "nargs", Value::int(1))?;
                }
            } else if !nargs.is_none() {
                let r = render_value_repr(_interp, &action)?;
                return Err(raise_optparse_exc(
                    _interp,
                    "OptionError",
                    &[
                        Value::string(format!("'nargs' must not be supplied for action {r}")),
                        self_val.0.clone(),
                    ],
                ));
            }
            Ok(Value::none())
        }

        fn _check_callback(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let action = _interp.get_attr(self_val.0, "action")?;
            let cb = _interp.get_attr(self_val.0, "callback")?;
            let cb_args = _interp.get_attr(self_val.0, "callback_args")?;
            let cb_kwargs = _interp.get_attr(self_val.0, "callback_kwargs")?;
            if action.as_str() == Some("callback") {
                if !is_callable_val(&cb) {
                    let r = render_value_repr(_interp, &cb)?;
                    return Err(raise_optparse_exc(
                        _interp,
                        "OptionError",
                        &[
                            Value::string(format!("callback not callable: {r}")),
                            self_val.0.clone(),
                        ],
                    ));
                }
                if !cb_args.is_none() && !matches!(cb_args.kind(), ValueKind::Tuple(_)) {
                    let r = render_value_repr(_interp, &cb_args)?;
                    return Err(raise_optparse_exc(
                        _interp,
                        "OptionError",
                        &[
                            Value::string(format!(
                                "callback_args, if supplied, must be a tuple: not {r}"
                            )),
                            self_val.0.clone(),
                        ],
                    ));
                }
                if !cb_kwargs.is_none() && !matches!(cb_kwargs.kind(), ValueKind::Dict(_)) {
                    let r = render_value_repr(_interp, &cb_kwargs)?;
                    return Err(raise_optparse_exc(
                        _interp,
                        "OptionError",
                        &[
                            Value::string(format!(
                                "callback_kwargs, if supplied, must be a dict: not {r}"
                            )),
                            self_val.0.clone(),
                        ],
                    ));
                }
            } else {
                if !cb.is_none() {
                    let r = render_value_repr(_interp, &cb)?;
                    return Err(raise_optparse_exc(
                        _interp,
                        "OptionError",
                        &[
                            Value::string(format!(
                                "callback supplied ({r}) for non-callback option"
                            )),
                            self_val.0.clone(),
                        ],
                    ));
                }
                if !cb_args.is_none() {
                    return Err(raise_optparse_exc(
                        _interp,
                        "OptionError",
                        &[
                            Value::string("callback_args supplied for non-callback option"),
                            self_val.0.clone(),
                        ],
                    ));
                }
                if !cb_kwargs.is_none() {
                    return Err(raise_optparse_exc(
                        _interp,
                        "OptionError",
                        &[
                            Value::string("callback_kwargs supplied for non-callback option"),
                            self_val.0.clone(),
                        ],
                    ));
                }
            }
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let so_attr = _interp.get_attr(self_val.0, "_short_opts")?;
            let mut all = list_of_strings(_interp, &so_attr)?;
            let lo_attr = _interp.get_attr(self_val.0, "_long_opts")?;
            all.extend(list_of_strings(_interp, &lo_attr)?);
            Ok(Value::string(all.join("/")))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            repr_optparse_obj(_interp, self_val.0)
        }

        fn takes_value(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let typ = _interp.get_attr(self_val.0, "type")?;
            Ok(Value::bool_(!typ.is_none()))
        }

        fn get_opt_string(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let lo_attr = _interp.get_attr(self_val.0, "_long_opts")?;
            let long_opts = list_of_strings(_interp, &lo_attr)?;
            if let Some(first) = long_opts.first() {
                return Ok(Value::string(first));
            }
            let so_attr = _interp.get_attr(self_val.0, "_short_opts")?;
            let short_opts = list_of_strings(_interp, &so_attr)?;
            Ok(Value::string(short_opts.first().map(String::as_str).unwrap_or("")))
        }

        fn check_value(
            #[positional_only] self_val: PyValueRef<'_>,
            opt: PyValue,
            value: PyValue,
        ) -> Result<Value> {
            let tc = _interp.get_attr(self_val.0, "TYPE_CHECKER")?;
            let typ = _interp.get_attr(self_val.0, "type")?;
            let checker = typ
                .as_str()
                .and_then(|t| dict_get_str(&tc, t));
            match checker {
                None => Ok(value.0),
                Some(chk) => call_pos(_interp, chk, &[self_val.0.clone(), opt.0, value.0]),
            }
        }

        fn convert_value(
            #[positional_only] self_val: PyValueRef<'_>,
            opt: PyValue,
            value: PyValue,
        ) -> Result<Value> {
            if value.0.is_none() {
                return Ok(Value::none());
            }
            let nargs = attr_i64(_interp, self_val.0, "nargs")?;
            let check_fn = _interp.get_attr(self_val.0, "check_value")?;
            if nargs == 1 {
                call_pos(_interp, check_fn, &[opt.0, value.0])
            } else {
                let items = _interp.collect_iterable(&value.0)?;
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    out.push(call_pos(_interp, check_fn.clone(), &[opt.0.clone(), item])?);
                }
                Ok(Value::tuple(out))
            }
        }

        fn process(
            #[positional_only] self_val: PyValueRef<'_>,
            opt: PyValue,
            value: PyValue,
            values: PyValue,
            parser: PyValue,
        ) -> Result<Value> {
            let convert_fn = _interp.get_attr(self_val.0, "convert_value")?;
            let converted = call_pos(_interp, convert_fn, &[opt.0.clone(), value.0])?;
            let action = _interp.get_attr(self_val.0, "action")?;
            let dest = _interp.get_attr(self_val.0, "dest")?;
            let take_fn = _interp.get_attr(self_val.0, "take_action")?;
            call_pos(
                _interp,
                take_fn,
                &[action, dest, opt.0, converted, values.0, parser.0],
            )
        }

        fn take_action(
            #[positional_only] self_val: PyValueRef<'_>,
            action: PyStr<'_>,
            dest: PyValue,
            opt: PyValue,
            value: PyValue,
            values: PyValue,
            parser: PyValue,
        ) -> Result<Value> {
            let dest_name = dest.0.as_str().unwrap_or("");
            match &*action {
                "store" => {
                    _interp.assign_attr(values.0, dest_name, value.0)?;
                }
                "store_const" => {
                    let c = _interp.get_attr(self_val.0, "const")?;
                    _interp.assign_attr(values.0, dest_name, c)?;
                }
                "store_true" => {
                    _interp.assign_attr(values.0, dest_name, Value::bool_(true))?;
                }
                "store_false" => {
                    _interp.assign_attr(values.0, dest_name, Value::bool_(false))?;
                }
                "append" => {
                    let ensure = _interp.get_attr(&values.0, "ensure_value")?;
                    let lst = call_pos(
                        _interp,
                        ensure,
                        &[Value::string(dest_name), Value::list(Vec::new())],
                    )?;
                    let app = _interp.get_attr(&lst, "append")?;
                    call_pos(_interp, app, &[value.0])?;
                }
                "append_const" => {
                    let c = _interp.get_attr(self_val.0, "const")?;
                    let ensure = _interp.get_attr(&values.0, "ensure_value")?;
                    let lst = call_pos(
                        _interp,
                        ensure,
                        &[Value::string(dest_name), Value::list(Vec::new())],
                    )?;
                    let app = _interp.get_attr(&lst, "append")?;
                    call_pos(_interp, app, &[c])?;
                }
                "count" => {
                    let ensure = _interp.get_attr(&values.0, "ensure_value")?;
                    let cur =
                        call_pos(_interp, ensure, &[Value::string(dest_name), Value::int(0)])?;
                    let next = _interp.eval_binary(cur, BinaryOp::Add, Value::int(1))?;
                    _interp.assign_attr(values.0, dest_name, next)?;
                }
                "callback" => {
                    let cb = _interp.get_attr(self_val.0, "callback")?;
                    let cb_args = _interp.get_attr(self_val.0, "callback_args")?;
                    let cb_kwargs = _interp.get_attr(self_val.0, "callback_kwargs")?;
                    let mut pos = vec![self_val.0.clone(), opt.0, value.0, parser.0];
                    if !cb_args.is_none() {
                        pos.extend(_interp.collect_iterable(&cb_args)?);
                    }
                    let mut kw = IndexMap::new();
                    if let Some(d) = cb_kwargs.as_dict() {
                        for (k, v) in d.iter() {
                            if let Some(ks) = pykey_as_str(k) {
                                kw.insert(ks.to_owned(), v.clone());
                            }
                        }
                    }
                    call_with_args_kwargs(_interp, cb, &pos, &kw)?;
                }
                "help" => {
                    let ph = _interp.get_attr(&parser.0, "print_help")?;
                    call_pos(_interp, ph, &[])?;
                    let ex = _interp.get_attr(&parser.0, "exit")?;
                    call_pos(_interp, ex, &[])?;
                }
                "version" => {
                    let pv = _interp.get_attr(&parser.0, "print_version")?;
                    call_pos(_interp, pv, &[])?;
                    let ex = _interp.get_attr(&parser.0, "exit")?;
                    call_pos(_interp, ex, &[])?;
                }
                other => {
                    return Err(PyError::named(
                        "ValueError",
                        format!("unknown action {other:?}"),
                    ));
                }
            }
            Ok(Value::int(1))
        }
    }

    // ── Values ───────────────────────────────────────────────────────────────

    class Values {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] defaults: OptValueRef<'_>,
        ) -> Result<Value> {
            if let Some(def) = defaults.as_value()
                && _interp.truthy_value(def)?
            {
                let items_fn = _interp.get_attr(def, "items")?;
                let pairs_val = call_pos(_interp, items_fn, &[])?;
                let pairs = _interp.collect_iterable(&pairs_val)?;
                for pair in pairs {
                    let elems = _interp.collect_iterable(&pair)?;
                    if elems.len() == 2
                        && let Some(k) = elems[0].as_str()
                    {
                        _interp.assign_attr(self_val.0.clone(), k, elems[1].clone())?;
                    }
                }
            }
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let d = instance_dict_value(_interp, self_val.0)?;
            Ok(Value::string(_interp.render_value_as_str(&d)?))
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            repr_optparse_obj(_interp, self_val.0)
        }

        fn __eq__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let values_cls = optparse_attr(_interp, "Values")?;
            let dict_cls = primitive_class_by_name("dict")
                .map(Value::py_class)
                .unwrap_or_else(|| Value::builtin_function("dict"));
            if py_isinstance(_interp, &other.0, &values_cls)? {
                let d1 = instance_dict_value(_interp, self_val.0)?;
                let d2 = instance_dict_value(_interp, &other.0)?;
                _interp.eval_binary(d1, BinaryOp::Eq, d2)
            } else if py_isinstance(_interp, &other.0, &dict_cls)? {
                let d1 = instance_dict_value(_interp, self_val.0)?;
                _interp.eval_binary(d1, BinaryOp::Eq, other.0)
            } else {
                Ok(Value::not_implemented())
            }
        }

        fn _update_careful(
            #[positional_only] self_val: PyValueRef<'_>,
            dict: PyValue,
        ) -> Result<Value> {
            let dir_fn = Value::builtin_function("dir");
            let names_val = call_pos(_interp, dir_fn, std::slice::from_ref(self_val.0))?;
            let names = list_of_strings(_interp, &names_val)?;
            for attr in names {
                if let Some(dval) = dict_get_str(&dict.0, &attr)
                    && !dval.is_none()
                {
                    _interp.assign_attr(self_val.0.clone(), &attr, dval)?;
                }
            }
            Ok(Value::none())
        }

        fn _update_loose(
            #[positional_only] self_val: PyValueRef<'_>,
            dict: PyValue,
        ) -> Result<Value> {
            let d = instance_dict_value(_interp, self_val.0)?;
            let upd = _interp.get_attr(&d, "update")?;
            call_pos(_interp, upd, &[dict.0])?;
            Ok(Value::none())
        }

        fn _update(
            #[positional_only] self_val: PyValueRef<'_>,
            dict: PyValue,
            mode: PyStr<'_>,
        ) -> Result<Value> {
            match &*mode {
                "careful" => {
                    let f = _interp.get_attr(self_val.0, "_update_careful")?;
                    call_pos(_interp, f, &[dict.0])
                }
                "loose" => {
                    let f = _interp.get_attr(self_val.0, "_update_loose")?;
                    call_pos(_interp, f, &[dict.0])
                }
                other => Err(PyError::named(
                    "ValueError",
                    format!("invalid update mode: {other:?}"),
                )),
            }
        }

        fn read_module(
            #[positional_only] self_val: PyValueRef<'_>,
            modname: PyStr<'_>,
            #[default("careful".into())] mode: PyStr<'_>,
        ) -> Result<Value> {
            let mod_val = _interp.load_module(&modname)?;
            let vars_fn = Value::builtin_function("vars");
            let mod_vars = call_pos(_interp, vars_fn, &[mod_val])?;
            let upd = _interp.get_attr(self_val.0, "_update")?;
            call_pos(_interp, upd, &[mod_vars, Value::string(&*mode)])
        }

        fn read_file(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyStr<'_>,
            #[default("careful".into())] mode: PyStr<'_>,
        ) -> Result<Value> {
            let source = std::fs::read_to_string(&*filename).map_err(|e| {
                PyError::named("OSError", format!("cannot read '{}': {e}", &*filename))
            })?;
            let ns = Value::dict(PyDict::default());
            _interp.exec_source(&source, Some(ns.clone()), None)?;
            let upd = _interp.get_attr(self_val.0, "_update")?;
            call_pos(_interp, upd, &[ns, Value::string(&*mode)])
        }

        fn ensure_value(
            #[positional_only] self_val: PyValueRef<'_>,
            attr: PyStr<'_>,
            value: PyValue,
        ) -> Result<Value> {
            let existing = match _interp.get_attr(self_val.0, &attr) {
                Ok(v) if !v.is_none() => Some(v),
                Ok(_) => None,
                Err(ref e) if e.class_name_is("AttributeError") => None,
                Err(e) => return Err(e),
            };
            if let Some(v) = existing {
                Ok(v)
            } else {
                _interp.assign_attr(self_val.0.clone(), &attr, value.0.clone())?;
                Ok(value.0)
            }
        }
    }

    // ── OptionContainer ──────────────────────────────────────────────────────

    class OptionContainer {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            option_class: PyValue,
            conflict_handler: PyValue,
            description: PyValue,
        ) -> Result<Value> {
            let create_list = _interp.get_attr(self_val.0, "_create_option_list")?;
            call_pos(_interp, create_list, &[])?;
            _interp.assign_attr(self_val.0.clone(), "option_class", option_class.0)?;
            let set_ch = _interp.get_attr(self_val.0, "set_conflict_handler")?;
            call_pos(_interp, set_ch, &[conflict_handler.0])?;
            let set_desc = _interp.get_attr(self_val.0, "set_description")?;
            call_pos(_interp, set_desc, &[description.0])?;
            Ok(Value::none())
        }

        fn _create_option_mappings(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "_short_opt", Value::dict(PyDict::default()))?;
            _interp.assign_attr(self_val.0.clone(), "_long_opt", Value::dict(PyDict::default()))?;
            _interp.assign_attr(self_val.0.clone(), "defaults", Value::dict(PyDict::default()))?;
            Ok(Value::none())
        }

        fn _share_option_mappings(
            #[positional_only] self_val: PyValueRef<'_>,
            parser: PyValue,
        ) -> Result<Value> {
            let so = _interp.get_attr(&parser.0, "_short_opt")?;
            let lo = _interp.get_attr(&parser.0, "_long_opt")?;
            let defs = _interp.get_attr(&parser.0, "defaults")?;
            _interp.assign_attr(self_val.0.clone(), "_short_opt", so)?;
            _interp.assign_attr(self_val.0.clone(), "_long_opt", lo)?;
            _interp.assign_attr(self_val.0.clone(), "defaults", defs)?;
            Ok(Value::none())
        }

        fn set_conflict_handler(
            #[positional_only] self_val: PyValueRef<'_>,
            handler: PyValue,
        ) -> Result<Value> {
            if handler.0.as_str() != Some("error") && handler.0.as_str() != Some("resolve") {
                let r = render_value_repr(_interp, &handler.0)?;
                return Err(PyError::named(
                    "ValueError",
                    format!("invalid conflict_resolution value {r}"),
                ));
            }
            _interp.assign_attr(self_val.0.clone(), "conflict_handler", handler.0)?;
            Ok(Value::none())
        }

        fn set_description(
            #[positional_only] self_val: PyValueRef<'_>,
            description: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "description", description.0)?;
            Ok(Value::none())
        }

        fn get_description(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.get_attr(self_val.0, "description")
        }

        fn destroy(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.delete_attr(self_val.0.clone(), "_short_opt")?;
            _interp.delete_attr(self_val.0.clone(), "_long_opt")?;
            _interp.delete_attr(self_val.0.clone(), "defaults")?;
            Ok(Value::none())
        }

        fn _check_conflict(
            #[positional_only] self_val: PyValueRef<'_>,
            option: PyValue,
        ) -> Result<Value> {
            let short_map = _interp.get_attr(self_val.0, "_short_opt")?;
            let long_map = _interp.get_attr(self_val.0, "_long_opt")?;
            let so_attr = _interp.get_attr(&option.0, "_short_opts")?;
            let short_opts = list_of_strings(_interp, &so_attr)?;
            let lo_attr = _interp.get_attr(&option.0, "_long_opts")?;
            let long_opts = list_of_strings(_interp, &lo_attr)?;

            let mut conflict_opts: Vec<(String, Value)> = Vec::new();
            for opt in &short_opts {
                if let Some(existing) = dict_get_str(&short_map, opt) {
                    conflict_opts.push((opt.clone(), existing));
                }
            }
            for opt in &long_opts {
                if let Some(existing) = dict_get_str(&long_map, opt) {
                    conflict_opts.push((opt.clone(), existing));
                }
            }

            if !conflict_opts.is_empty() {
                let handler = _interp.get_attr(self_val.0, "conflict_handler")?;
                match handler.as_str() {
                    Some("error") => {
                        let names: Vec<&str> =
                            conflict_opts.iter().map(|(s, _)| s.as_str()).collect();
                        return Err(raise_optparse_exc(
                            _interp,
                            "OptionConflictError",
                            &[
                                Value::string(format!(
                                    "conflicting option string(s): {}",
                                    names.join(", ")
                                )),
                                option.0,
                            ],
                        ));
                    }
                    Some("resolve") => {
                        for (opt_str, c_opt) in conflict_opts {
                            if opt_str.starts_with("--") {
                                let lo = _interp.get_attr(&c_opt, "_long_opts")?;
                                list_remove_first(_interp, &lo, &Value::string(&opt_str))?;
                                dict_remove_str(_interp, &long_map, &opt_str)?;
                            } else {
                                let so = _interp.get_attr(&c_opt, "_short_opts")?;
                                list_remove_first(_interp, &so, &Value::string(&opt_str))?;
                                dict_remove_str(_interp, &short_map, &opt_str)?;
                            }
                            let rem_s = _interp.get_attr(&c_opt, "_short_opts")?;
                            let rem_l = _interp.get_attr(&c_opt, "_long_opts")?;
                            if !_interp.truthy_value(&rem_s)? && !_interp.truthy_value(&rem_l)? {
                                let container = _interp.get_attr(&c_opt, "container")?;
                                let olist = _interp.get_attr(&container, "option_list")?;
                                list_remove_first(_interp, &olist, &c_opt)?;
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Value::none())
        }

        fn add_option(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let option = if args.first().is_some_and(|a| a.as_str().is_some()) {
                let opt_cls = _interp.get_attr(self_val.0, "option_class")?;
                call_with_args_kwargs(_interp, opt_cls, args, &kwargs)?
            } else if args.len() == 1 && kwargs.is_empty() {
                let candidate = args[0].clone();
                let opt_cls = optparse_attr(_interp, "Option")?;
                if !py_isinstance(_interp, &candidate, &opt_cls)? {
                    let r = render_value_repr(_interp, &candidate)?;
                    return Err(PyError::named(
                        "TypeError",
                        format!("not an Option instance: {r}"),
                    ));
                }
                candidate
            } else {
                return Err(PyError::named("TypeError", "invalid arguments"));
            };

            let chk = _interp.get_attr(self_val.0, "_check_conflict")?;
            call_pos(_interp, chk, std::slice::from_ref(&option))?;

            let olist = _interp.get_attr(self_val.0, "option_list")?;
            let app = _interp.get_attr(&olist, "append")?;
            call_pos(_interp, app, std::slice::from_ref(&option))?;
            _interp.assign_attr(option.clone(), "container", self_val.0.clone())?;

            let short_map = _interp.get_attr(self_val.0, "_short_opt")?;
            let long_map = _interp.get_attr(self_val.0, "_long_opt")?;
            let so_attr = _interp.get_attr(&option, "_short_opts")?;
            for opt in list_of_strings(_interp, &so_attr)? {
                short_map.dict_insert(PyKey::str_from(opt), option.clone())?;
            }
            let lo_attr = _interp.get_attr(&option, "_long_opts")?;
            for opt in list_of_strings(_interp, &lo_attr)? {
                long_map.dict_insert(PyKey::str_from(opt), option.clone())?;
            }

            let dest = _interp.get_attr(&option, "dest")?;
            if let Some(dest_str) = dest.as_str() {
                let defaults = _interp.get_attr(self_val.0, "defaults")?;
                let opt_default = _interp.get_attr(&option, "default")?;
                if !is_no_default(_interp, &opt_default) {
                    defaults.dict_insert(PyKey::str_from(dest_str), opt_default)?;
                } else if !dict_contains_str(&defaults, dest_str) {
                    defaults.dict_insert(PyKey::str_from(dest_str), Value::none())?;
                }
            }
            Ok(option)
        }

        fn add_options(
            #[positional_only] self_val: PyValueRef<'_>,
            option_list: PyValue,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(&option_list.0)?;
            let add_fn = _interp.get_attr(self_val.0, "add_option")?;
            for opt in items {
                call_pos(_interp, add_fn.clone(), &[opt])?;
            }
            Ok(Value::none())
        }

        fn get_option(
            #[positional_only] self_val: PyValueRef<'_>,
            opt_str: PyStr<'_>,
        ) -> Result<Value> {
            let short_map = _interp.get_attr(self_val.0, "_short_opt")?;
            if let Some(v) = dict_get_str(&short_map, &opt_str) {
                return Ok(v);
            }
            let long_map = _interp.get_attr(self_val.0, "_long_opt")?;
            Ok(dict_get_str(&long_map, &opt_str).unwrap_or_else(Value::none))
        }

        fn has_option(
            #[positional_only] self_val: PyValueRef<'_>,
            opt_str: PyStr<'_>,
        ) -> Result<Value> {
            let short_map = _interp.get_attr(self_val.0, "_short_opt")?;
            let long_map = _interp.get_attr(self_val.0, "_long_opt")?;
            Ok(Value::bool_(
                dict_contains_str(&short_map, &opt_str) || dict_contains_str(&long_map, &opt_str),
            ))
        }

        fn remove_option(
            #[positional_only] self_val: PyValueRef<'_>,
            opt_str: PyStr<'_>,
        ) -> Result<Value> {
            let short_map = _interp.get_attr(self_val.0, "_short_opt")?;
            let long_map = _interp.get_attr(self_val.0, "_long_opt")?;
            let option = dict_get_str(&short_map, &opt_str)
                .or_else(|| dict_get_str(&long_map, &opt_str));
            let Some(option) = option else {
                let r = render_value_repr(_interp, &Value::string(&*opt_str))?;
                return Err(PyError::named("ValueError", format!("no such option {r}")));
            };
            let so_attr = _interp.get_attr(&option, "_short_opts")?;
            for opt in list_of_strings(_interp, &so_attr)? {
                dict_remove_str(_interp, &short_map, &opt)?;
            }
            let lo_attr = _interp.get_attr(&option, "_long_opts")?;
            for opt in list_of_strings(_interp, &lo_attr)? {
                dict_remove_str(_interp, &long_map, &opt)?;
            }
            let container = _interp.get_attr(&option, "container")?;
            let olist = _interp.get_attr(&container, "option_list")?;
            list_remove_first(_interp, &olist, &option)?;
            Ok(Value::none())
        }

        fn format_option_help(
            #[positional_only] self_val: PyValueRef<'_>,
            formatter: PyValue,
        ) -> Result<Value> {
            let olist_attr = _interp.get_attr(self_val.0, "option_list")?;
            let olist = _interp.collect_iterable(&olist_attr)?;
            if olist.is_empty() {
                return Ok(Value::string(""));
            }
            let fmt_opt = _interp.get_attr(&formatter.0, "format_option")?;
            let mut out = String::new();
            for opt in olist {
                let help = _interp.get_attr(&opt, "help")?;
                if help.as_str() != Some(SUPPRESS_HELP_STR) {
                    let s = call_pos(_interp, fmt_opt.clone(), &[opt])?;
                    if let Some(txt) = s.as_str() {
                        out.push_str(txt);
                    }
                }
            }
            Ok(Value::string(out))
        }

        fn format_description(
            #[positional_only] self_val: PyValueRef<'_>,
            formatter: PyValue,
        ) -> Result<Value> {
            let get_desc = _interp.get_attr(self_val.0, "get_description")?;
            let desc = call_pos(_interp, get_desc, &[])?;
            let fmt_desc = _interp.get_attr(&formatter.0, "format_description")?;
            call_pos(_interp, fmt_desc, &[desc])
        }

        fn format_help(
            #[positional_only] self_val: PyValueRef<'_>,
            formatter: PyValue,
        ) -> Result<Value> {
            let mut parts: Vec<String> = Vec::new();
            let desc = _interp.get_attr(self_val.0, "description")?;
            if _interp.truthy_value(&desc)? {
                let fd = _interp.get_attr(self_val.0, "format_description")?;
                let res = call_pos(_interp, fd, std::slice::from_ref(&formatter.0))?;
                parts.push(res.as_str().unwrap_or("").to_owned());
            }
            let olist = _interp.get_attr(self_val.0, "option_list")?;
            if _interp.truthy_value(&olist)? {
                let foh = _interp.get_attr(self_val.0, "format_option_help")?;
                let res = call_pos(_interp, foh, &[formatter.0])?;
                parts.push(res.as_str().unwrap_or("").to_owned());
            }
            Ok(Value::string(parts.join("\n")))
        }
    }

    // ── OptionGroup ──────────────────────────────────────────────────────────

    class OptionGroup("OptionContainer") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            parser: PyValue,
            title: PyValue,
            #[default(OptionalArg::Omitted)] description: OptValueRef<'_>,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "parser", parser.0.clone())?;
            let oc_cls = optparse_attr(_interp, "OptionContainer")?;
            let oc_init = _interp.get_attr(&oc_cls, "__init__")?;
            let opt_cls = _interp.get_attr(&parser.0, "option_class")?;
            let ch = _interp.get_attr(&parser.0, "conflict_handler")?;
            let desc = description.as_value().cloned().unwrap_or_else(Value::none);
            call_pos(
                _interp,
                oc_init,
                &[self_val.0.clone(), opt_cls, ch, desc],
            )?;
            _interp.assign_attr(self_val.0.clone(), "title", title.0)?;
            Ok(Value::none())
        }

        fn _create_option_list(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "option_list", Value::list(Vec::new()))?;
            let parser = _interp.get_attr(self_val.0, "parser")?;
            let share = _interp.get_attr(self_val.0, "_share_option_mappings")?;
            call_pos(_interp, share, &[parser])
        }

        fn set_title(#[positional_only] self_val: PyValueRef<'_>, title: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "title", title.0)?;
            Ok(Value::none())
        }

        fn destroy(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let oc_cls = optparse_attr(_interp, "OptionContainer")?;
            let oc_destroy = _interp.get_attr(&oc_cls, "destroy")?;
            call_pos(_interp, oc_destroy, std::slice::from_ref(self_val.0))?;
            _interp.delete_attr(self_val.0.clone(), "option_list")?;
            Ok(Value::none())
        }

        fn format_help(
            #[positional_only] self_val: PyValueRef<'_>,
            formatter: PyValue,
        ) -> Result<Value> {
            let title = _interp.get_attr(self_val.0, "title")?;
            let fh = _interp.get_attr(&formatter.0, "format_heading")?;
            let heading = call_pos(_interp, fh, &[title])?;
            let mut out = heading.as_str().unwrap_or("").to_owned();
            let ind = _interp.get_attr(&formatter.0, "indent")?;
            call_pos(_interp, ind, &[])?;
            let oc_cls = optparse_attr(_interp, "OptionContainer")?;
            let oc_fh = _interp.get_attr(&oc_cls, "format_help")?;
            let body = call_pos(_interp, oc_fh, &[self_val.0.clone(), formatter.0.clone()])?;
            out.push_str(body.as_str().unwrap_or(""));
            let ded = _interp.get_attr(&formatter.0, "dedent")?;
            call_pos(_interp, ded, &[])?;
            Ok(Value::string(out))
        }
    }

    // ── OptionParser ─────────────────────────────────────────────────────────

    class OptionParser("OptionContainer") {
        constants {
            "standard_option_list" => Value::list(Vec::new()),
        }

        #[allow(clippy::too_many_arguments)]
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] usage: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] option_list: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] option_class: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] version: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] conflict_handler: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] description: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] formatter: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] add_help_option: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] prog: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] epilog: OptValueRef<'_>,
        ) -> Result<Value> {
            let opt_cls = match option_class.as_value() {
                Some(v) => v.clone(),
                None => optparse_attr(_interp, "Option")?,
            };
            let ch = conflict_handler
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::string("error"));
            let desc = description.as_value().cloned().unwrap_or_else(Value::none);

            let oc_cls = optparse_attr(_interp, "OptionContainer")?;
            let oc_init = _interp.get_attr(&oc_cls, "__init__")?;
            call_pos(_interp, oc_init, &[self_val.0.clone(), opt_cls, ch, desc])?;

            let set_usage = _interp.get_attr(self_val.0, "set_usage")?;
            call_pos(
                _interp,
                set_usage,
                &[usage.as_value().cloned().unwrap_or_else(Value::none)],
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "prog",
                prog.as_value().cloned().unwrap_or_else(Value::none),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "version",
                version.as_value().cloned().unwrap_or_else(Value::none),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "allow_interspersed_args",
                Value::bool_(true),
            )?;
            _interp.assign_attr(
                self_val.0.clone(),
                "process_default_values",
                Value::bool_(true),
            )?;

            let fmt = match formatter.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => {
                    let ihf = optparse_attr(_interp, "IndentedHelpFormatter")?;
                    call_pos(_interp, ihf, &[])?
                }
            };
            _interp.assign_attr(self_val.0.clone(), "formatter", fmt.clone())?;
            let set_parser = _interp.get_attr(&fmt, "set_parser")?;
            call_pos(_interp, set_parser, std::slice::from_ref(self_val.0))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "epilog",
                epilog.as_value().cloned().unwrap_or_else(Value::none),
            )?;

            let add_help = match add_help_option.as_value() {
                Some(v) => v.clone(),
                None => Value::bool_(true),
            };
            let pop_fn = _interp.get_attr(self_val.0, "_populate_option_list")?;
            call_pos(
                _interp,
                pop_fn,
                &[
                    option_list.as_value().cloned().unwrap_or_else(Value::none),
                    add_help,
                ],
            )?;

            let init_state = _interp.get_attr(self_val.0, "_init_parsing_state")?;
            call_pos(_interp, init_state, &[])?;
            Ok(Value::none())
        }

        fn destroy(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let oc_cls = optparse_attr(_interp, "OptionContainer")?;
            let oc_destroy = _interp.get_attr(&oc_cls, "destroy")?;
            call_pos(_interp, oc_destroy, std::slice::from_ref(self_val.0))?;
            let groups_attr = _interp.get_attr(self_val.0, "option_groups")?;
            let groups = _interp.collect_iterable(&groups_attr)?;
            for g in groups {
                let gd = _interp.get_attr(&g, "destroy")?;
                call_pos(_interp, gd, &[])?;
            }
            _interp.delete_attr(self_val.0.clone(), "option_list")?;
            _interp.delete_attr(self_val.0.clone(), "option_groups")?;
            _interp.delete_attr(self_val.0.clone(), "formatter")?;
            Ok(Value::none())
        }

        fn _create_option_list(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "option_list", Value::list(Vec::new()))?;
            _interp.assign_attr(self_val.0.clone(), "option_groups", Value::list(Vec::new()))?;
            let create_maps = _interp.get_attr(self_val.0, "_create_option_mappings")?;
            call_pos(_interp, create_maps, &[])
        }

        fn _add_help_option(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let add_opt = _interp.get_attr(self_val.0, "add_option")?;
            let mut kw = IndexMap::new();
            kw.insert("action".to_string(), Value::string("help"));
            kw.insert(
                "help".to_string(),
                Value::string("show this help message and exit"),
            );
            call_with_args_kwargs(
                _interp,
                add_opt,
                &[Value::string("-h"), Value::string("--help")],
                &kw,
            )?;
            Ok(Value::none())
        }

        fn _add_version_option(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let add_opt = _interp.get_attr(self_val.0, "add_option")?;
            let mut kw = IndexMap::new();
            kw.insert("action".to_string(), Value::string("version"));
            kw.insert(
                "help".to_string(),
                Value::string("show program's version number and exit"),
            );
            call_with_args_kwargs(_interp, add_opt, &[Value::string("--version")], &kw)?;
            Ok(Value::none())
        }

        fn _populate_option_list(
            #[positional_only] self_val: PyValueRef<'_>,
            option_list: PyValue,
            #[default(OptionalArg::Omitted)] add_help: OptValueRef<'_>,
        ) -> Result<Value> {
            let std_opts = _interp.get_attr(self_val.0, "standard_option_list")?;
            if _interp.truthy_value(&std_opts)? {
                let ao = _interp.get_attr(self_val.0, "add_options")?;
                call_pos(_interp, ao, &[std_opts])?;
            }
            if _interp.truthy_value(&option_list.0)? {
                let ao = _interp.get_attr(self_val.0, "add_options")?;
                call_pos(_interp, ao, &[option_list.0])?;
            }
            let version = _interp.get_attr(self_val.0, "version")?;
            if _interp.truthy_value(&version)? {
                let av = _interp.get_attr(self_val.0, "_add_version_option")?;
                call_pos(_interp, av, &[])?;
            }
            let do_help = match add_help.as_value() {
                Some(v) => _interp.truthy_value(v)?,
                None => true,
            };
            if do_help {
                let ah = _interp.get_attr(self_val.0, "_add_help_option")?;
                call_pos(_interp, ah, &[])?;
            }
            Ok(Value::none())
        }

        fn _init_parsing_state(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "rargs", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "largs", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "values", Value::none())?;
            Ok(Value::none())
        }

        fn set_usage(#[positional_only] self_val: PyValueRef<'_>, usage: PyValue) -> Result<Value> {
            let new_usage = if usage.0.is_none() {
                Value::string("%prog [options]")
            } else if usage.0.as_str() == Some(SUPPRESS_USAGE_STR) {
                Value::none()
            } else if let Some(s) = usage.0.as_str() {
                if s.len() >= 7 && s[..7].eq_ignore_ascii_case("usage: ") {
                    Value::string(&s[7..])
                } else {
                    usage.0
                }
            } else {
                usage.0
            };
            _interp.assign_attr(self_val.0.clone(), "usage", new_usage)?;
            Ok(Value::none())
        }

        fn enable_interspersed_args(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(
                self_val.0.clone(),
                "allow_interspersed_args",
                Value::bool_(true),
            )?;
            Ok(Value::none())
        }

        fn disable_interspersed_args(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            _interp.assign_attr(
                self_val.0.clone(),
                "allow_interspersed_args",
                Value::bool_(false),
            )?;
            Ok(Value::none())
        }

        fn set_process_default_values(
            #[positional_only] self_val: PyValueRef<'_>,
            process: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "process_default_values", process.0)?;
            Ok(Value::none())
        }

        fn set_default(
            #[positional_only] self_val: PyValueRef<'_>,
            dest: PyStr<'_>,
            value: PyValue,
        ) -> Result<Value> {
            let defaults = _interp.get_attr(self_val.0, "defaults")?;
            defaults.dict_insert(PyKey::str_from(&*dest), value.0)?;
            Ok(Value::none())
        }

        fn set_defaults(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let defaults = _interp.get_attr(self_val.0, "defaults")?;
            for (k, v) in kwargs {
                defaults.dict_insert(PyKey::str_from(k), v)?;
            }
            Ok(Value::none())
        }

        fn _get_all_options(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let olist_attr = _interp.get_attr(self_val.0, "option_list")?;
            let mut options = _interp.collect_iterable(&olist_attr)?;
            let groups_attr = _interp.get_attr(self_val.0, "option_groups")?;
            let groups = _interp.collect_iterable(&groups_attr)?;
            for g in groups {
                let g_olist = _interp.get_attr(&g, "option_list")?;
                options.extend(_interp.collect_iterable(&g_olist)?);
            }
            Ok(Value::list(options))
        }

        fn get_default_values(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let values_cls = optparse_attr(_interp, "Values")?;
            let defaults = _interp.get_attr(self_val.0, "defaults")?;
            let pdv_attr = _interp.get_attr(self_val.0, "process_default_values")?;
            let process = _interp.truthy_value(&pdv_attr)?;
            if !process {
                return call_pos(_interp, values_cls, &[defaults]);
            }
            let copy_fn = _interp.get_attr(&defaults, "copy")?;
            let defs_copy = call_pos(_interp, copy_fn, &[])?;
            let get_all = _interp.get_attr(self_val.0, "_get_all_options")?;
            let all_val = call_pos(_interp, get_all, &[])?;
            let all_opts = _interp.collect_iterable(&all_val)?;
            for opt in all_opts {
                let dest = _interp.get_attr(&opt, "dest")?;
                if let Some(dest_str) = dest.as_str()
                    && let Some(def_val) = dict_get_str(&defs_copy, dest_str)
                    && def_val.as_str().is_some()
                {
                    let gos = _interp.get_attr(&opt, "get_opt_string")?;
                    let opt_str = call_pos(_interp, gos, &[])?;
                    let cv = _interp.get_attr(&opt, "check_value")?;
                    let checked = call_pos(_interp, cv, &[opt_str, def_val])?;
                    defs_copy.dict_insert(PyKey::str_from(dest_str), checked)?;
                }
            }
            call_pos(_interp, values_cls, &[defs_copy])
        }

        fn add_option_group(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[varkwargs] kwargs: IndexMap<String, Value>,
        ) -> Result<Value> {
            let group = if args.first().is_some_and(|a| a.as_str().is_some()) {
                let grp_cls = optparse_attr(_interp, "OptionGroup")?;
                let mut pos = Vec::with_capacity(args.len() + 1);
                pos.push(self_val.0.clone());
                pos.extend_from_slice(args);
                call_with_args_kwargs(_interp, grp_cls, &pos, &kwargs)?
            } else if args.len() == 1 && kwargs.is_empty() {
                let candidate = args[0].clone();
                let grp_cls = optparse_attr(_interp, "OptionGroup")?;
                if !py_isinstance(_interp, &candidate, &grp_cls)? {
                    let r = render_value_repr(_interp, &candidate)?;
                    return Err(PyError::named(
                        "TypeError",
                        format!("not an OptionGroup instance: {r}"),
                    ));
                }
                let grp_parser = _interp.get_attr(&candidate, "parser")?;
                if grp_parser.object_id() != self_val.0.object_id() {
                    return Err(PyError::named(
                        "ValueError",
                        "invalid OptionGroup (wrong parser)",
                    ));
                }
                candidate
            } else {
                return Err(PyError::named("TypeError", "invalid arguments"));
            };

            let groups = _interp.get_attr(self_val.0, "option_groups")?;
            let app = _interp.get_attr(&groups, "append")?;
            call_pos(_interp, app, std::slice::from_ref(&group))?;
            Ok(group)
        }

        fn get_option_group(
            #[positional_only] self_val: PyValueRef<'_>,
            opt_str: PyStr<'_>,
        ) -> Result<Value> {
            let short_map = _interp.get_attr(self_val.0, "_short_opt")?;
            let long_map = _interp.get_attr(self_val.0, "_long_opt")?;
            let opt = dict_get_str(&short_map, &opt_str)
                .or_else(|| dict_get_str(&long_map, &opt_str));
            if let Some(opt) = opt {
                let container = _interp.get_attr(&opt, "container")?;
                if container.object_id() != self_val.0.object_id() {
                    return Ok(container);
                }
            }
            Ok(Value::none())
        }

        fn _get_args(#[positional_only] _self_val: PyValueRef<'_>, args: PyValue) -> Result<Value> {
            if args.0.is_none() {
                let sys_mod = _interp.load_module("sys")?;
                let argv = _interp.get_attr(&sys_mod, "argv")?;
                let items = _interp.collect_iterable(&argv)?;
                let rest = if items.len() > 1 {
                    items[1..].to_vec()
                } else {
                    Vec::new()
                };
                Ok(Value::list(rest))
            } else {
                let items = _interp.collect_iterable(&args.0)?;
                Ok(Value::list(items))
            }
        }

        fn parse_args(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] args: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] values: OptValueRef<'_>,
        ) -> Result<Value> {
            let get_args = _interp.get_attr(self_val.0, "_get_args")?;
            let rargs = call_pos(
                _interp,
                get_args,
                &[args.as_value().cloned().unwrap_or_else(Value::none)],
            )?;
            let values_obj = match values.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => {
                    let gdv = _interp.get_attr(self_val.0, "get_default_values")?;
                    call_pos(_interp, gdv, &[])?
                }
            };
            let largs = Value::list(Vec::new());
            _interp.assign_attr(self_val.0.clone(), "rargs", rargs.clone())?;
            _interp.assign_attr(self_val.0.clone(), "largs", largs.clone())?;
            _interp.assign_attr(self_val.0.clone(), "values", values_obj.clone())?;

            let proc_args = _interp.get_attr(self_val.0, "_process_args")?;
            if let Err(err) = call_pos(
                _interp,
                proc_args,
                &[largs.clone(), rargs.clone(), values_obj.clone()],
            ) {
                let bad_cls = optparse_attr(_interp, "BadOptionError")?;
                let val_cls = optparse_attr(_interp, "OptionValueError")?;
                let err_val = _interp.materialize_pyerror(err.clone())?;
                if py_isinstance(_interp, &err_val, &Value::tuple(vec![bad_cls, val_cls]))? {
                    let msg = _interp.render_value_as_str(&err_val)?;
                    let error_fn = _interp.get_attr(self_val.0, "error")?;
                    call_pos(_interp, error_fn, &[Value::string(msg)])?;
                } else {
                    return Err(err);
                }
            }

            let mut out_args = _interp.collect_iterable(&largs)?;
            out_args.extend(_interp.collect_iterable(&rargs)?);
            let check_vals = _interp.get_attr(self_val.0, "check_values")?;
            call_pos(_interp, check_vals, &[values_obj, Value::list(out_args)])
        }

        fn check_values(
            #[positional_only] _self_val: PyValueRef<'_>,
            values: PyValue,
            args: PyValue,
        ) -> Result<Value> {
            Ok(Value::tuple(vec![values.0, args.0]))
        }

        fn _process_args(
            #[positional_only] self_val: PyValueRef<'_>,
            largs: PyValue,
            rargs: PyValue,
            values: PyValue,
        ) -> Result<Value> {
            while _interp.truthy_value(&rargs.0)? {
                let arg_val = _interp.eval_index(&rargs.0, Value::int(0))?;
                let arg = arg_val.as_str().unwrap_or("").to_owned();
                if arg == "--" {
                    list_pop_zero(_interp, &rargs.0)?;
                    return Ok(Value::none());
                } else if arg.starts_with("--") {
                    let pl = _interp.get_attr(self_val.0, "_process_long_opt")?;
                    call_pos(_interp, pl, &[rargs.0.clone(), values.0.clone()])?;
                } else if arg.starts_with('-') && arg.len() > 1 {
                    let ps = _interp.get_attr(self_val.0, "_process_short_opts")?;
                    call_pos(_interp, ps, &[rargs.0.clone(), values.0.clone()])?;
                } else {
                    let allow_inter = _interp.get_attr(self_val.0, "allow_interspersed_args")?;
                    if _interp.truthy_value(&allow_inter)? {
                        let app = _interp.get_attr(&largs.0, "append")?;
                        call_pos(_interp, app, &[arg_val])?;
                        list_pop_zero(_interp, &rargs.0)?;
                    } else {
                        return Ok(Value::none());
                    }
                }
            }
            Ok(Value::none())
        }

        fn _match_long_opt(
            #[positional_only] self_val: PyValueRef<'_>,
            opt: PyStr<'_>,
        ) -> Result<Value> {
            let long_map = _interp.get_attr(self_val.0, "_long_opt")?;
            match_abbrev_impl(_interp, &opt, &long_map)
        }

        fn _process_long_opt(
            #[positional_only] self_val: PyValueRef<'_>,
            rargs: PyValue,
            values: PyValue,
        ) -> Result<Value> {
            let pop_fn = _interp.get_attr(&rargs.0, "pop")?;
            let arg_val = call_pos(_interp, pop_fn.clone(), &[Value::int(0)])?;
            let arg = arg_val.as_str().unwrap_or("").to_owned();

            let (raw_opt, had_explicit_value) = if let Some((left, right)) = arg.split_once('=') {
                let ins_fn = _interp.get_attr(&rargs.0, "insert")?;
                call_pos(_interp, ins_fn, &[Value::int(0), Value::string(right)])?;
                (left.to_owned(), true)
            } else {
                (arg, false)
            };

            let match_fn = _interp.get_attr(self_val.0, "_match_long_opt")?;
            let opt_val = call_pos(_interp, match_fn, &[Value::string(raw_opt)])?;
            let opt = opt_val.as_str().unwrap_or("").to_owned();
            let long_map = _interp.get_attr(self_val.0, "_long_opt")?;
            let option = _interp.eval_index(&long_map, opt_val.clone())?;

            let takes_fn = _interp.get_attr(&option, "takes_value")?;
            let takes_res = call_pos(_interp, takes_fn, &[])?;
            let takes_val = _interp.truthy_value(&takes_res)?;

            let value = if takes_val {
                let nargs = attr_i64(_interp, &option, "nargs")?.max(0) as usize;
                let rlen = _interp.collect_iterable(&rargs.0)?.len();
                if rlen < nargs {
                    let msg = if nargs == 1 {
                        format!("{opt} option requires 1 argument")
                    } else {
                        format!("{opt} option requires {nargs} arguments")
                    };
                    let error_fn = _interp.get_attr(self_val.0, "error")?;
                    call_pos(_interp, error_fn, &[Value::string(msg)])?;
                    return Ok(Value::none());
                } else if nargs == 1 {
                    call_pos(_interp, pop_fn, &[Value::int(0)])?
                } else {
                    let mut collected = Vec::with_capacity(nargs);
                    for _ in 0..nargs {
                        collected.push(call_pos(_interp, pop_fn.clone(), &[Value::int(0)])?);
                    }
                    Value::tuple(collected)
                }
            } else if had_explicit_value {
                let error_fn = _interp.get_attr(self_val.0, "error")?;
                call_pos(
                    _interp,
                    error_fn,
                    &[Value::string(format!("{opt} option does not take a value"))],
                )?;
                return Ok(Value::none());
            } else {
                Value::none()
            };

            let proc_fn = _interp.get_attr(&option, "process")?;
            call_pos(_interp, proc_fn, &[opt_val, value, values.0, self_val.0.clone()])?;
            Ok(Value::none())
        }

        fn _process_short_opts(
            #[positional_only] self_val: PyValueRef<'_>,
            rargs: PyValue,
            values: PyValue,
        ) -> Result<Value> {
            let pop_fn = _interp.get_attr(&rargs.0, "pop")?;
            let arg_val = call_pos(_interp, pop_fn.clone(), &[Value::int(0)])?;
            let arg = arg_val.as_str().unwrap_or("").to_owned();
            let chars: Vec<char> = arg.chars().collect();
            let short_map = _interp.get_attr(self_val.0, "_short_opt")?;

            let mut stop = false;
            let mut i = 1usize;
            while i < chars.len() {
                let ch = chars[i];
                let opt = format!("-{ch}");
                i += 1;
                let Some(option) = dict_get_str(&short_map, &opt) else {
                    return Err(raise_optparse_exc(
                        _interp,
                        "BadOptionError",
                        &[Value::string(opt)],
                    ));
                };
                let takes_fn = _interp.get_attr(&option, "takes_value")?;
                let takes_res = call_pos(_interp, takes_fn, &[])?;
                let takes_val = _interp.truthy_value(&takes_res)?;
                let value = if takes_val {
                    if i < chars.len() {
                        let rest: String = chars[i..].iter().collect();
                        let ins_fn = _interp.get_attr(&rargs.0, "insert")?;
                        call_pos(_interp, ins_fn, &[Value::int(0), Value::string(rest)])?;
                        stop = true;
                    }
                    let nargs = attr_i64(_interp, &option, "nargs")?.max(0) as usize;
                    let rlen = _interp.collect_iterable(&rargs.0)?.len();
                    if rlen < nargs {
                        let msg = if nargs == 1 {
                            format!("{opt} option requires 1 argument")
                        } else {
                            format!("{opt} option requires {nargs} arguments")
                        };
                        let error_fn = _interp.get_attr(self_val.0, "error")?;
                        call_pos(_interp, error_fn, &[Value::string(msg)])?;
                        return Ok(Value::none());
                    } else if nargs == 1 {
                        call_pos(_interp, pop_fn.clone(), &[Value::int(0)])?
                    } else {
                        let mut collected = Vec::with_capacity(nargs);
                        for _ in 0..nargs {
                            collected.push(call_pos(_interp, pop_fn.clone(), &[Value::int(0)])?);
                        }
                        Value::tuple(collected)
                    }
                } else {
                    Value::none()
                };

                let proc_fn = _interp.get_attr(&option, "process")?;
                call_pos(
                    _interp,
                    proc_fn,
                    &[Value::string(opt), value, values.0.clone(), self_val.0.clone()],
                )?;
                if stop {
                    break;
                }
            }
            Ok(Value::none())
        }

        fn get_prog_name(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let prog = _interp.get_attr(self_val.0, "prog")?;
            if prog.is_none() {
                let sys_mod = _interp.load_module("sys")?;
                let argv = _interp.get_attr(&sys_mod, "argv")?;
                let items = _interp.collect_iterable(&argv)?;
                let argv0 = items
                    .first()
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let os_path = _interp.load_module("os.path")?;
                let basename = _interp.get_attr(&os_path, "basename")?;
                call_pos(_interp, basename, &[Value::string(argv0)])
            } else {
                Ok(prog)
            }
        }

        fn expand_prog_name(
            #[positional_only] self_val: PyValueRef<'_>,
            s: PyStr<'_>,
        ) -> Result<Value> {
            let gpn = _interp.get_attr(self_val.0, "get_prog_name")?;
            let prog = call_pos(_interp, gpn, &[])?;
            let prog_s = prog.as_str().unwrap_or("");
            Ok(Value::string(s.replace("%prog", prog_s)))
        }

        fn get_description(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let desc = _interp.get_attr(self_val.0, "description")?;
            if desc.is_none() {
                return Ok(Value::none());
            }
            let epn = _interp.get_attr(self_val.0, "expand_prog_name")?;
            call_pos(_interp, epn, &[desc])
        }

        fn exit(
            #[positional_only] _self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] status: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] msg: OptValueRef<'_>,
        ) -> Result<Value> {
            if let Some(m) = msg.as_value()
                && _interp.truthy_value(m)?
            {
                let sys_mod = _interp.load_module("sys")?;
                let stderr = _interp.get_attr(&sys_mod, "stderr")?;
                let write_fn = _interp.get_attr(&stderr, "write")?;
                call_pos(_interp, write_fn, std::slice::from_ref(m))?;
            }
            let status_val = status
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::int(0));
            let sys_mod = _interp.load_module("sys")?;
            let exit_fn = _interp.get_attr(&sys_mod, "exit")?;
            call_pos(_interp, exit_fn, &[status_val])
        }

        fn error(#[positional_only] self_val: PyValueRef<'_>, msg: PyValue) -> Result<Value> {
            let sys_mod = _interp.load_module("sys")?;
            let stderr = _interp.get_attr(&sys_mod, "stderr")?;
            let pu = _interp.get_attr(self_val.0, "print_usage")?;
            call_pos(_interp, pu, &[stderr])?;
            let gpn = _interp.get_attr(self_val.0, "get_prog_name")?;
            let prog = call_pos(_interp, gpn, &[])?;
            let prog_s = prog.as_str().unwrap_or("");
            let msg_s = _interp.render_value_as_str(&msg.0)?;
            let exit_fn = _interp.get_attr(self_val.0, "exit")?;
            call_pos(
                _interp,
                exit_fn,
                &[
                    Value::int(2),
                    Value::string(format!("{prog_s}: error: {msg_s}\n")),
                ],
            )
        }

        fn get_usage(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let usage = _interp.get_attr(self_val.0, "usage")?;
            if _interp.truthy_value(&usage)? {
                let epn = _interp.get_attr(self_val.0, "expand_prog_name")?;
                let expanded = call_pos(_interp, epn, &[usage])?;
                let fmt = _interp.get_attr(self_val.0, "formatter")?;
                let fu = _interp.get_attr(&fmt, "format_usage")?;
                call_pos(_interp, fu, &[expanded])
            } else {
                Ok(Value::string(""))
            }
        }

        fn print_usage(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] file: OptValueRef<'_>,
        ) -> Result<Value> {
            let usage = _interp.get_attr(self_val.0, "usage")?;
            if _interp.truthy_value(&usage)? {
                let gu = _interp.get_attr(self_val.0, "get_usage")?;
                let text = call_pos(_interp, gu, &[])?;
                let s = text.as_str().unwrap_or("");
                write_to_file_or_stdout(_interp, file.as_value(), &format!("{s}\n"))?;
            }
            Ok(Value::none())
        }

        fn get_version(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let version = _interp.get_attr(self_val.0, "version")?;
            if _interp.truthy_value(&version)? {
                let epn = _interp.get_attr(self_val.0, "expand_prog_name")?;
                call_pos(_interp, epn, &[version])
            } else {
                Ok(Value::string(""))
            }
        }

        fn print_version(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] file: OptValueRef<'_>,
        ) -> Result<Value> {
            let version = _interp.get_attr(self_val.0, "version")?;
            if _interp.truthy_value(&version)? {
                let gv = _interp.get_attr(self_val.0, "get_version")?;
                let text = call_pos(_interp, gv, &[])?;
                let s = text.as_str().unwrap_or("");
                write_to_file_or_stdout(_interp, file.as_value(), &format!("{s}\n"))?;
            }
            Ok(Value::none())
        }

        fn format_option_help(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] formatter: OptValueRef<'_>,
        ) -> Result<Value> {
            let fmt = match formatter.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => _interp.get_attr(self_val.0, "formatter")?,
            };
            let sos = _interp.get_attr(&fmt, "store_option_strings")?;
            call_pos(_interp, sos, std::slice::from_ref(self_val.0))?;
            let mut result: Vec<String> = Vec::new();
            let fh = _interp.get_attr(&fmt, "format_heading")?;
            let heading = call_pos(_interp, fh, &[Value::string("Options")])?;
            result.push(heading.as_str().unwrap_or("").to_owned());
            let ind = _interp.get_attr(&fmt, "indent")?;
            call_pos(_interp, ind, &[])?;
            let olist = _interp.get_attr(self_val.0, "option_list")?;
            if _interp.truthy_value(&olist)? {
                let oc_cls = optparse_attr(_interp, "OptionContainer")?;
                let oc_foh = _interp.get_attr(&oc_cls, "format_option_help")?;
                let body = call_pos(_interp, oc_foh, &[self_val.0.clone(), fmt.clone()])?;
                result.push(body.as_str().unwrap_or("").to_owned());
                result.push("\n".to_string());
            }
            let groups_attr = _interp.get_attr(self_val.0, "option_groups")?;
            let groups = _interp.collect_iterable(&groups_attr)?;
            for grp in groups {
                let gfh = _interp.get_attr(&grp, "format_help")?;
                let gbody = call_pos(_interp, gfh, std::slice::from_ref(&fmt))?;
                result.push(gbody.as_str().unwrap_or("").to_owned());
                result.push("\n".to_string());
            }
            let ded = _interp.get_attr(&fmt, "dedent")?;
            call_pos(_interp, ded, &[])?;
            if !result.is_empty() {
                result.pop();
            }
            Ok(Value::string(result.concat()))
        }

        fn format_epilog(
            #[positional_only] self_val: PyValueRef<'_>,
            formatter: PyValue,
        ) -> Result<Value> {
            let epilog = _interp.get_attr(self_val.0, "epilog")?;
            let fe = _interp.get_attr(&formatter.0, "format_epilog")?;
            call_pos(_interp, fe, &[epilog])
        }

        fn format_help(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] formatter: OptValueRef<'_>,
        ) -> Result<Value> {
            let fmt = match formatter.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => _interp.get_attr(self_val.0, "formatter")?,
            };
            let mut parts: Vec<String> = Vec::new();
            let usage = _interp.get_attr(self_val.0, "usage")?;
            if _interp.truthy_value(&usage)? {
                let gu = _interp.get_attr(self_val.0, "get_usage")?;
                let u = call_pos(_interp, gu, &[])?;
                parts.push(format!("{}\n", u.as_str().unwrap_or("")));
            }
            let desc = _interp.get_attr(self_val.0, "description")?;
            if _interp.truthy_value(&desc)? {
                let fd = _interp.get_attr(self_val.0, "format_description")?;
                let d = call_pos(_interp, fd, std::slice::from_ref(&fmt))?;
                parts.push(format!("{}\n", d.as_str().unwrap_or("")));
            }
            let foh = _interp.get_attr(self_val.0, "format_option_help")?;
            let oh = call_pos(_interp, foh, std::slice::from_ref(&fmt))?;
            parts.push(oh.as_str().unwrap_or("").to_owned());
            let fe = _interp.get_attr(self_val.0, "format_epilog")?;
            let ep = call_pos(_interp, fe, &[fmt])?;
            parts.push(ep.as_str().unwrap_or("").to_owned());
            Ok(Value::string(parts.concat()))
        }

        fn print_help(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] file: OptValueRef<'_>,
        ) -> Result<Value> {
            let fh = _interp.get_attr(self_val.0, "format_help")?;
            let text = call_pos(_interp, fh, &[])?;
            write_to_file_or_stdout(_interp, file.as_value(), text.as_str().unwrap_or(""))?;
            Ok(Value::none())
        }
    }
}
