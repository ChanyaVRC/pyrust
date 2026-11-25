// `pstats` module — 100% native Rust implementation of Python 3.12's
// `pstats` module (issue #3247).
//
// Provides `SortKey` (`enum.StrEnum`), `FunctionProfile`, `StatsProfile`,
// `Stats`, `TupleComp`, helper functions (`func_strip_path`,
// `func_get_function_name`, `func_std_string`, `add_func_stats`,
// `add_callers`, `count_calls`, `f8`), and a self-contained CPython-compatible
// marshal encoder/decoder for `dump_stats` / `Stats(filename)`.
//
// Reference: <https://docs.python.org/3/library/profile.html#module-pstats>

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyNumber, PyPath, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{ExpandedCallArg, Interpreter, render_value_repr, value_class};
use crate::value::{PyDict, PyKey, PySet, Value, ValueKind};

fn val_as_f64(v: &Value) -> Option<f64> {
    match v.kind() {
        ValueKind::Float(f) => Some(f),
        ValueKind::Int(n) => Some(n as f64),
        _ => None,
    }
}

// ── Self-contained CPython marshal (v2 writer + v0-v4 reader) for stats dicts ──

const TYPE_NULL: u8 = b'0';
const TYPE_NONE: u8 = b'N';
const TYPE_FALSE: u8 = b'F';
const TYPE_TRUE: u8 = b'T';
const TYPE_INT: u8 = b'i';
const TYPE_INT64: u8 = b'I';
const TYPE_FLOAT: u8 = b'f';
const TYPE_BINARY_FLOAT: u8 = b'g';
const TYPE_STRING: u8 = b's';
const TYPE_INTERNED: u8 = b't';
const TYPE_UNICODE: u8 = b'u';
const TYPE_ASCII: u8 = b'a';
const TYPE_ASCII_INTERNED: u8 = b'A';
const TYPE_SHORT_ASCII: u8 = b'z';
const TYPE_SHORT_ASCII_INTERNED: u8 = b'Z';
const TYPE_TUPLE: u8 = b'(';
const TYPE_SMALL_TUPLE: u8 = b')';
const TYPE_LIST: u8 = b'[';
const TYPE_DICT: u8 = b'{';
const TYPE_REF: u8 = b'r';
const FLAG_REF: u8 = 0x80;

fn write_marshal_value(val: &Value, out: &mut Vec<u8>) -> Result<()> {
    match val.kind() {
        ValueKind::None => out.push(TYPE_NONE),
        ValueKind::Bool(false) => out.push(TYPE_FALSE),
        ValueKind::Bool(true) => out.push(TYPE_TRUE),
        ValueKind::Int(n) => {
            if let Ok(n32) = i32::try_from(n) {
                out.push(TYPE_INT);
                out.extend_from_slice(&n32.to_le_bytes());
            } else {
                out.push(TYPE_INT64);
                out.extend_from_slice(&n.to_le_bytes());
            }
        }
        ValueKind::Float(f) => {
            out.push(TYPE_BINARY_FLOAT);
            out.extend_from_slice(&f.to_le_bytes());
        }
        ValueKind::Str(s) => {
            let bytes = s.as_bytes();
            let len = u32::try_from(bytes.len())
                .map_err(|_| PyError::named("ValueError", "string too long for marshal"))?;
            out.push(TYPE_UNICODE);
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(bytes);
        }
        ValueKind::Tuple(items) => {
            let len = u32::try_from(items.len())
                .map_err(|_| PyError::named("ValueError", "tuple too long for marshal"))?;
            out.push(TYPE_TUPLE);
            out.extend_from_slice(&len.to_le_bytes());
            for item in items {
                write_marshal_value(item, out)?;
            }
        }
        ValueKind::List(items) => {
            let len = u32::try_from(items.len())
                .map_err(|_| PyError::named("ValueError", "list too long for marshal"))?;
            out.push(TYPE_LIST);
            out.extend_from_slice(&len.to_le_bytes());
            for item in items.iter() {
                write_marshal_value(item, out)?;
            }
        }
        ValueKind::Dict(d) => {
            out.push(TYPE_DICT);
            for (k, v) in d.iter() {
                let kv = pykey_to_value(k);
                write_marshal_value(&kv, out)?;
                write_marshal_value(v, out)?;
            }
            out.push(TYPE_NULL);
        }
        _ => {
            return Err(PyError::named("ValueError", "unmarshallable object"));
        }
    }
    Ok(())
}

fn pykey_to_value(k: &PyKey) -> Value {
    match k {
        PyKey::Int(n) => Value::int(*n),
        PyKey::Str(v) => v.clone(),
        PyKey::Tuple(items) => Value::tuple(items.iter().map(pykey_to_value).collect()),
        PyKey::Bytes(b) => Value::bytes(b.to_vec()),
        PyKey::None => Value::none(),
        PyKey::BigInt(bi) => Value::bigint((**bi).clone()),
        PyKey::Float(bits) => Value::float(f64::from_bits(*bits)),
        PyKey::FrozenSet(items) => {
            Value::tuple(items.items_rc().iter().map(pykey_to_value).collect())
        }
        PyKey::Bool(b) => Value::bool_(*b),
        PyKey::Ellipsis => Value::ellipsis(),
        PyKey::Complex(re, im) => Value::complex(*re, *im),
        PyKey::Object { value, .. } => value.clone(),
    }
}

struct MarshalReader<'a> {
    data: &'a [u8],
    pos: usize,
    refs: Vec<Value>,
}

impl<'a> MarshalReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            refs: Vec::new(),
        }
    }

    fn read_u8(&mut self) -> Result<u8> {
        if self.pos >= self.data.len() {
            return Err(PyError::named("EOFError", "EOF read where object expected"));
        }
        let b = self.data[self.pos];
        self.pos += 1;
        Ok(b)
    }

    fn read_exact(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.pos + n > self.data.len() {
            return Err(PyError::named("EOFError", "EOF read where object expected"));
        }
        let slice = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(slice)
    }

    fn read_i32(&mut self) -> Result<i32> {
        let b = self.read_exact(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_u32(&mut self) -> Result<u32> {
        let b = self.read_exact(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn read_i64(&mut self) -> Result<i64> {
        let b = self.read_exact(8)?;
        Ok(i64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_f64(&mut self) -> Result<f64> {
        let b = self.read_exact(8)?;
        Ok(f64::from_le_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }

    fn read_object(&mut self) -> Result<Option<Value>> {
        let code = self.read_u8()?;
        let flag = (code & FLAG_REF) != 0;
        let kind = code & !FLAG_REF;

        if kind == TYPE_NULL {
            return Ok(None);
        }
        if kind == TYPE_REF {
            let idx = self.read_u32()? as usize;
            let val = self.refs.get(idx).cloned().ok_or_else(|| {
                PyError::named("ValueError", "bad marshal data (invalid reference)")
            })?;
            return Ok(Some(val));
        }

        let ref_idx = if flag {
            let idx = self.refs.len();
            self.refs.push(Value::none());
            Some(idx)
        } else {
            None
        };

        let val = match kind {
            TYPE_NONE => Value::none(),
            TYPE_FALSE => Value::bool_(false),
            TYPE_TRUE => Value::bool_(true),
            TYPE_INT => Value::int(i64::from(self.read_i32()?)),
            TYPE_INT64 => Value::int(self.read_i64()?),
            TYPE_BINARY_FLOAT => Value::float(self.read_f64()?),
            TYPE_FLOAT => {
                let n = self.read_u8()? as usize;
                let s = std::str::from_utf8(self.read_exact(n)?)
                    .map_err(|_| PyError::named("ValueError", "bad marshal data (float)"))?;
                let f = s
                    .parse::<f64>()
                    .map_err(|_| PyError::named("ValueError", "bad marshal data (float)"))?;
                Value::float(f)
            }
            TYPE_UNICODE | TYPE_INTERNED | TYPE_ASCII | TYPE_ASCII_INTERNED => {
                let n = self.read_u32()? as usize;
                let bytes = self.read_exact(n)?;
                let s = String::from_utf8_lossy(bytes).into_owned();
                Value::string(s)
            }
            TYPE_STRING => {
                let n = self.read_u32()? as usize;
                let bytes = self.read_exact(n)?;
                // In marshal v0-v2, str/bytes may use 's'; stats keys are strings.
                let s = String::from_utf8_lossy(bytes).into_owned();
                Value::string(s)
            }
            TYPE_SHORT_ASCII | TYPE_SHORT_ASCII_INTERNED => {
                let n = self.read_u8()? as usize;
                let bytes = self.read_exact(n)?;
                let s = String::from_utf8_lossy(bytes).into_owned();
                Value::string(s)
            }
            TYPE_TUPLE => {
                let n = self.read_u32()? as usize;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    let item = self.read_object()?.ok_or_else(|| {
                        PyError::named("ValueError", "bad marshal data (NULL in tuple)")
                    })?;
                    items.push(item);
                }
                Value::tuple(items)
            }
            TYPE_SMALL_TUPLE => {
                let n = self.read_u8()? as usize;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    let item = self.read_object()?.ok_or_else(|| {
                        PyError::named("ValueError", "bad marshal data (NULL in tuple)")
                    })?;
                    items.push(item);
                }
                Value::tuple(items)
            }
            TYPE_LIST => {
                let n = self.read_u32()? as usize;
                let mut items = Vec::with_capacity(n);
                for _ in 0..n {
                    let item = self.read_object()?.ok_or_else(|| {
                        PyError::named("ValueError", "bad marshal data (NULL in list)")
                    })?;
                    items.push(item);
                }
                Value::list(items)
            }
            TYPE_DICT => {
                let mut dict = PyDict::default();
                loop {
                    let Some(k) = self.read_object()? else {
                        break;
                    };
                    let v = self.read_object()?.ok_or_else(|| {
                        PyError::named("ValueError", "bad marshal data (NULL dict value)")
                    })?;
                    let pyk = value_to_pykey(&k)?;
                    dict.insert(pyk, v);
                }
                Value::dict(dict)
            }
            _ => {
                return Err(PyError::named(
                    "ValueError",
                    format!("bad marshal data (unknown type code {kind:#x})"),
                ));
            }
        };

        if let Some(idx) = ref_idx {
            self.refs[idx] = val.clone();
        }
        Ok(Some(val))
    }
}

fn value_to_pykey(v: &Value) -> Result<PyKey> {
    match v.kind() {
        ValueKind::Int(n) => Ok(PyKey::Int(n)),
        ValueKind::Bool(b) => Ok(PyKey::Int(i64::from(b))),
        ValueKind::Str(_) => Ok(PyKey::Str(v.clone())),
        ValueKind::Bytes(b) => Ok(PyKey::Bytes(b.clone())),
        ValueKind::None => Ok(PyKey::None),
        ValueKind::Float(f) => Ok(PyKey::Float(f.to_bits())),
        ValueKind::Tuple(items) => {
            let mut keys = Vec::with_capacity(items.len());
            for item in items {
                keys.push(value_to_pykey(item)?);
            }
            Ok(PyKey::Tuple(keys.into()))
        }
        _ => Err(PyError::named("TypeError", "unhashable type in stats data")),
    }
}

pub(crate) fn marshal_dump_stats(stats: &Value) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    write_marshal_value(stats, &mut buf)?;
    Ok(buf)
}

pub(crate) fn marshal_load_stats(interp: &mut Interpreter, data: &[u8]) -> Result<Value> {
    let mut reader = MarshalReader::new(data);
    match reader.read_object() {
        Ok(Some(v)) => Ok(v),
        Ok(None) => Err(PyError::named("EOFError", "EOF read where object expected")),
        Err(marshal_err) => {
            // Fallback to pickle.loads if the file was pickled.
            if let Ok(pickle_mod) = interp.load_module("pickle")
                && let Ok(loads_fn) = interp.get_attr(&pickle_mod, "loads")
                && let Ok(v) = call_pos(interp, loads_fn, &[Value::bytes(data.to_vec())])
            {
                return Ok(v);
            }
            Err(marshal_err)
        }
    }
}

// ── Shared helpers ───────────────────────────────────────────────────────────

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(
    interp: &mut Interpreter,
    obj: &Value,
    method: &str,
    args: &[Value],
) -> Result<Value> {
    let f = interp.get_attr(obj, method)?;
    call_pos(interp, f, args)
}

fn pstats_attr(interp: &mut Interpreter, name: &str) -> Result<Value> {
    let m = interp.load_module("pstats")?;
    interp.get_attr(&m, name)
}

fn write_stream(interp: &mut Interpreter, stream: &Value, text: &str) -> Result<()> {
    call_method(interp, stream, "write", &[Value::string(text)])?;
    Ok(())
}

fn f8_str(x: f64) -> String {
    format!("{x:8.3}")
}

fn extract_func_triple(v: &Value) -> Option<(String, i64, String)> {
    let t = v.as_tuple()?;
    if t.len() != 3 {
        return None;
    }
    let f = t[0].as_str()?.to_string();
    let l = t[1].as_int()?;
    let n = t[2].as_str()?.to_string();
    Some((f, l, n))
}

fn func_std_string_rs(filename: &str, line: i64, name: &str) -> String {
    if filename == "~" && line == 0 {
        if name.starts_with('<') && name.ends_with('>') {
            format!("{{{}}}", &name[1..name.len() - 1])
        } else {
            name.to_string()
        }
    } else {
        format!("{filename}:{line}({name})")
    }
}

fn func_strip_path_rs(interp: &mut Interpreter, func: &Value) -> Result<Value> {
    let items = interp.collect_iterable(func)?;
    if items.len() != 3 {
        return Err(PyError::named("ValueError", "func_name must be a 3-tuple"));
    }
    let os_path = interp.load_module("os.path")?;
    let basename = interp.get_attr(&os_path, "basename")?;
    let base = call_pos(interp, basename, &[items[0].clone()])?;
    Ok(Value::tuple(vec![base, items[1].clone(), items[2].clone()]))
}

fn add_callers_rs(interp: &mut Interpreter, target: &Value, source: &Value) -> Result<Value> {
    let new_callers = Value::dict(PyDict::default());
    for src in [target, source] {
        let items_val = call_method(interp, src, "items", &[])?;
        let pairs = interp.collect_iterable(&items_val)?;
        for pair in pairs {
            let kv = interp.collect_iterable(&pair)?;
            if kv.len() != 2 {
                continue;
            }
            let func = &kv[0];
            let caller = &kv[1];
            let has = call_method(
                interp,
                &new_callers,
                "__contains__",
                std::slice::from_ref(func),
            )?;
            if interp.truthy_value(&has)? {
                let prev = interp.eval_index(&new_callers, func.clone())?;
                let combined = if let (Some(ct), Some(pt)) = (caller.as_tuple(), prev.as_tuple()) {
                    let mut out = Vec::with_capacity(ct.len().min(pt.len()));
                    for (a, b) in ct.iter().zip(pt.iter()) {
                        out.push(interp.eval_binary(
                            a.clone(),
                            crate::ast::BinaryOp::Add,
                            b.clone(),
                        )?);
                    }
                    Value::tuple(out)
                } else {
                    interp.eval_binary(prev, crate::ast::BinaryOp::Add, caller.clone())?
                };
                call_method(
                    interp,
                    &new_callers,
                    "__setitem__",
                    &[func.clone(), combined],
                )?;
            } else {
                call_method(
                    interp,
                    &new_callers,
                    "__setitem__",
                    &[func.clone(), caller.clone()],
                )?;
            }
        }
    }
    Ok(new_callers)
}

fn add_func_stats_rs(interp: &mut Interpreter, target: &Value, source: &Value) -> Result<Value> {
    let t = interp.collect_iterable(target)?;
    let s = interp.collect_iterable(source)?;
    if t.len() != 5 || s.len() != 5 {
        return Err(PyError::named(
            "ValueError",
            "stats tuple must have length 5",
        ));
    }
    let cc = interp.eval_binary(s[0].clone(), crate::ast::BinaryOp::Add, t[0].clone())?;
    let nc = interp.eval_binary(s[1].clone(), crate::ast::BinaryOp::Add, t[1].clone())?;
    let tt = interp.eval_binary(s[2].clone(), crate::ast::BinaryOp::Add, t[2].clone())?;
    let ct = interp.eval_binary(s[3].clone(), crate::ast::BinaryOp::Add, t[3].clone())?;
    let callers = add_callers_rs(interp, &t[4], &s[4])?;
    Ok(Value::tuple(vec![cc, nc, tt, ct, callers]))
}

#[derive(Clone)]
struct SortArgDef {
    specs: Vec<(usize, i8)>,
    label: &'static str,
}

fn sort_arg_defs_table() -> IndexMap<&'static str, SortArgDef> {
    let mut m = IndexMap::new();
    m.insert(
        "calls",
        SortArgDef {
            specs: vec![(1, -1)],
            label: "call count",
        },
    );
    m.insert(
        "ncalls",
        SortArgDef {
            specs: vec![(1, -1)],
            label: "call count",
        },
    );
    m.insert(
        "cumtime",
        SortArgDef {
            specs: vec![(3, -1)],
            label: "cumulative time",
        },
    );
    m.insert(
        "cumulative",
        SortArgDef {
            specs: vec![(3, -1)],
            label: "cumulative time",
        },
    );
    m.insert(
        "filename",
        SortArgDef {
            specs: vec![(4, 1)],
            label: "file name",
        },
    );
    m.insert(
        "line",
        SortArgDef {
            specs: vec![(5, 1)],
            label: "line number",
        },
    );
    m.insert(
        "module",
        SortArgDef {
            specs: vec![(4, 1)],
            label: "file name",
        },
    );
    m.insert(
        "name",
        SortArgDef {
            specs: vec![(6, 1)],
            label: "function name",
        },
    );
    m.insert(
        "nfl",
        SortArgDef {
            specs: vec![(6, 1), (4, 1), (5, 1)],
            label: "name/file/line",
        },
    );
    m.insert(
        "pcalls",
        SortArgDef {
            specs: vec![(0, -1)],
            label: "primitive call count",
        },
    );
    m.insert(
        "stdname",
        SortArgDef {
            specs: vec![(7, 1)],
            label: "standard name",
        },
    );
    m.insert(
        "time",
        SortArgDef {
            specs: vec![(2, -1)],
            label: "internal time",
        },
    );
    m.insert(
        "tottime",
        SortArgDef {
            specs: vec![(2, -1)],
            label: "internal time",
        },
    );
    m
}

fn expand_sort_abbrevs() -> HashMap<String, SortArgDef> {
    let defaults = sort_arg_defs_table();
    let mut map: HashMap<String, SortArgDef> = HashMap::new();
    let mut bad: HashSet<String> = HashSet::new();
    for (word, def) in defaults {
        let mut frag = word.to_string();
        while !frag.is_empty() {
            if map.contains_key(&frag) {
                bad.insert(frag);
                break;
            }
            map.insert(frag.clone(), def.clone());
            frag.pop();
        }
    }
    for b in bad {
        map.remove(&b);
    }
    map
}

fn build_sort_arg_dict_default() -> Value {
    let mut d = PyDict::default();
    for (word, def) in sort_arg_defs_table() {
        let specs: Vec<Value> = def
            .specs
            .iter()
            .map(|(idx, dir)| {
                Value::tuple(vec![Value::int(*idx as i64), Value::int(i64::from(*dir))])
            })
            .collect();
        d.insert(
            PyKey::str_from(word),
            Value::tuple(vec![Value::tuple(specs), Value::string(def.label)]),
        );
    }
    Value::dict(d)
}

/// Install `SortKey` (genuine `enum.StrEnum` subclass with value aliases matching
/// CPython 3.12) onto `pstats`.
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };
    let Ok(enum_mod) = interp.load_module("enum") else {
        return;
    };
    let Ok(str_enum_cls) = interp.get_attr(&enum_mod, "StrEnum") else {
        return;
    };
    let members = [
        ("CALLS", "calls"),
        ("CUMULATIVE", "cumulative"),
        ("FILENAME", "filename"),
        ("LINE", "line"),
        ("NAME", "name"),
        ("NFL", "nfl"),
        ("PCALLS", "pcalls"),
        ("STDNAME", "stdname"),
        ("TIME", "time"),
    ];
    let member_pairs: Vec<Value> = members
        .iter()
        .map(|(k, v)| Value::tuple(vec![Value::string(*k), Value::string(*v)]))
        .collect();
    let call_args = [
        ExpandedCallArg {
            name: None,
            value: Value::string("SortKey"),
        },
        ExpandedCallArg {
            name: None,
            value: Value::list(member_pairs),
        },
        ExpandedCallArg {
            name: Some("module".to_string()),
            value: Value::string("pstats"),
        },
    ];
    if let Ok(sort_key_cls) = interp.call_function_expanded(str_enum_cls, &call_args) {
        // Register CPython multi-value aliases ('ncalls', 'cumtime', 'module', 'tottime')
        // in `_value2member_map_` so `SortKey('ncalls') is SortKey.CALLS`, etc.
        if let Ok(v2m) = interp.get_attr(&sort_key_cls, "_value2member_map_") {
            let extra_aliases = [
                ("CALLS", "ncalls"),
                ("CUMULATIVE", "cumtime"),
                ("FILENAME", "module"),
                ("TIME", "tottime"),
            ];
            for (member_name, alias_val) in extra_aliases {
                if let Ok(member) = interp.get_attr(&sort_key_cls, member_name) {
                    let _ = call_method(
                        interp,
                        &v2m,
                        "__setitem__",
                        &[Value::string(alias_val), member],
                    );
                }
            }
        }
        mod_rc
            .borrow_mut()
            .insert_attr("SortKey".to_string(), sort_key_cls);
    }
}

pyrust_module! {
    constants {
        "__all__" => Value::list(vec![
            Value::string("Stats"),
            Value::string("SortKey"),
            Value::string("FunctionProfile"),
            Value::string("StatsProfile"),
        ]),
    }

    fn func_strip_path(func_name: PyValue) -> Result<Value> {
        func_strip_path_rs(_interp, &func_name.0)
    }

    fn func_get_function_name(func: PyValue) -> Result<Value> {
        _interp.eval_index(&func.0, Value::int(2))
    }

    fn func_std_string(func_name: PyValue) -> Result<Value> {
        if let Some((f, l, n)) = extract_func_triple(&func_name.0) {
            return Ok(Value::string(func_std_string_rs(&f, l, &n)));
        }
        let items = _interp.collect_iterable(&func_name.0)?;
        if items.len() != 3 {
            return Err(PyError::named("ValueError", "func_name must be a 3-tuple"));
        }
        let f = _interp.render_value_as_str(&items[0])?;
        let l = items[1].as_int().unwrap_or(0);
        let n = _interp.render_value_as_str(&items[2])?;
        Ok(Value::string(func_std_string_rs(&f, l, &n)))
    }

    fn add_func_stats(target: PyValue, source: PyValue) -> Result<Value> {
        add_func_stats_rs(_interp, &target.0, &source.0)
    }

    fn add_callers(target: PyValue, source: PyValue) -> Result<Value> {
        add_callers_rs(_interp, &target.0, &source.0)
    }

    fn count_calls(callers: PyValue) -> Result<Value> {
        let vals = call_method(_interp, &callers.0, "values", &[])?;
        let mut nc = 0i64;
        for c in _interp.collect_iterable(&vals)? {
            nc += c.as_int().unwrap_or(0);
        }
        Ok(Value::int(nc))
    }

    fn f8(x: PyNumber) -> Result<Value> {
        Ok(Value::string(f8_str(x.as_f64())))
    }

    class FunctionProfile {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            ncalls: PyValue,
            tottime: PyValue,
            percall_tottime: PyValue,
            cumtime: PyValue,
            percall_cumtime: PyValue,
            file_name: PyValue,
            line_number: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "ncalls", ncalls.0)?;
            _interp.assign_attr(self_val.0.clone(), "tottime", tottime.0)?;
            _interp.assign_attr(self_val.0.clone(), "percall_tottime", percall_tottime.0)?;
            _interp.assign_attr(self_val.0.clone(), "cumtime", cumtime.0)?;
            _interp.assign_attr(self_val.0.clone(), "percall_cumtime", percall_cumtime.0)?;
            _interp.assign_attr(self_val.0.clone(), "file_name", file_name.0)?;
            _interp.assign_attr(self_val.0.clone(), "line_number", line_number.0)?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let fields = [
                "ncalls",
                "tottime",
                "percall_tottime",
                "cumtime",
                "percall_cumtime",
                "file_name",
                "line_number",
            ];
            let mut parts = Vec::with_capacity(fields.len());
            for f in fields {
                let v = _interp.get_attr(self_val.0, f)?;
                let r = render_value_repr(_interp, &v)?;
                parts.push(format!("{f}={r}"));
            }
            Ok(Value::string(format!("FunctionProfile({})", parts.join(", "))))
        }

        fn __eq__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let my_cls = value_class(self_val.0);
            let other_cls = value_class(&other.0);
            if !crate::interpreter::values_are_identical(&my_cls, &other_cls) {
                return Ok(Value::not_implemented());
            }
            for f in [
                "ncalls",
                "tottime",
                "percall_tottime",
                "cumtime",
                "percall_cumtime",
                "file_name",
                "line_number",
            ] {
                let a = _interp.get_attr(self_val.0, f)?;
                let b = _interp.get_attr(&other.0, f)?;
                let eq = _interp.eval_binary(a, crate::ast::BinaryOp::Eq, b)?;
                if !_interp.truthy_value(&eq)? {
                    return Ok(Value::bool_(false));
                }
            }
            Ok(Value::bool_(true))
        }
    }

    class StatsProfile {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            total_tt: PyValue,
            func_profiles: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "total_tt", total_tt.0)?;
            _interp.assign_attr(self_val.0.clone(), "func_profiles", func_profiles.0)?;
            Ok(Value::none())
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let tt = _interp.get_attr(self_val.0, "total_tt")?;
            let fp = _interp.get_attr(self_val.0, "func_profiles")?;
            let tt_r = render_value_repr(_interp, &tt)?;
            let fp_r = render_value_repr(_interp, &fp)?;
            Ok(Value::string(format!(
                "StatsProfile(total_tt={tt_r}, func_profiles={fp_r})"
            )))
        }

        fn __eq__(#[positional_only] self_val: PyValueRef<'_>, other: PyValue) -> Result<Value> {
            let my_cls = value_class(self_val.0);
            let other_cls = value_class(&other.0);
            if !crate::interpreter::values_are_identical(&my_cls, &other_cls) {
                return Ok(Value::not_implemented());
            }
            for f in ["total_tt", "func_profiles"] {
                let a = _interp.get_attr(self_val.0, f)?;
                let b = _interp.get_attr(&other.0, f)?;
                let eq = _interp.eval_binary(a, crate::ast::BinaryOp::Eq, b)?;
                if !_interp.truthy_value(&eq)? {
                    return Ok(Value::bool_(false));
                }
            }
            Ok(Value::bool_(true))
        }
    }

    class TupleComp {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            comp_select_list: PyValue,
        ) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "comp_select_list", comp_select_list.0)?;
            Ok(Value::none())
        }

        fn compare(
            #[positional_only] self_val: PyValueRef<'_>,
            left: PyValue,
            right: PyValue,
        ) -> Result<Value> {
            let list = _interp.get_attr(self_val.0, "comp_select_list")?;
            for entry in _interp.collect_iterable(&list)? {
                let pair = _interp.collect_iterable(&entry)?;
                if pair.len() != 2 {
                    continue;
                }
                let idx = &pair[0];
                let dir = pair[1].as_int().unwrap_or(1);
                let l = _interp.eval_index(&left.0, idx.clone())?;
                let r = _interp.eval_index(&right.0, idx.clone())?;
                let lt = _interp.eval_binary(l.clone(), crate::ast::BinaryOp::Lt, r.clone())?;
                if _interp.truthy_value(&lt)? {
                    return Ok(Value::int(-dir));
                }
                let gt = _interp.eval_binary(l, crate::ast::BinaryOp::Gt, r)?;
                if _interp.truthy_value(&gt)? {
                    return Ok(Value::int(dir));
                }
            }
            Ok(Value::int(0))
        }
    }

    class Stats {
        constants {
            "sort_arg_dict_default" => build_sort_arg_dict_default(),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] args: &[Value],
            #[default(OptionalArg::Omitted)] stream: OptValueRef<'_>,
        ) -> Result<Value> {
            let stream_val = match stream.as_value().filter(|v| !v.is_none()) {
                Some(v) => v.clone(),
                None => {
                    let sys_mod = _interp.load_module("sys")?;
                    _interp.get_attr(&sys_mod, "stdout")?
                }
            };
            _interp.assign_attr(self_val.0.clone(), "stream", stream_val)?;
            let (first, rest) = if args.is_empty() {
                (Value::none(), args)
            } else {
                (args[0].clone(), &args[1..])
            };
            call_method(_interp, self_val.0, "init", &[first])?;
            if !rest.is_empty() {
                call_method(_interp, self_val.0, "add", rest)?;
            }
            Ok(Value::none())
        }

        fn init(#[positional_only] self_val: PyValueRef<'_>, arg: PyValue) -> Result<Value> {
            _interp.assign_attr(self_val.0.clone(), "all_callees", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "files", Value::list(Vec::new()))?;
            _interp.assign_attr(self_val.0.clone(), "fcn_list", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "total_tt", Value::float(0.0))?;
            _interp.assign_attr(self_val.0.clone(), "total_calls", Value::int(0))?;
            _interp.assign_attr(self_val.0.clone(), "prim_calls", Value::int(0))?;
            _interp.assign_attr(self_val.0.clone(), "max_name_len", Value::int(0))?;
            _interp.assign_attr(self_val.0.clone(), "top_level", Value::set(PySet::default()))?;
            _interp.assign_attr(self_val.0.clone(), "stats", Value::dict(PyDict::default()))?;
            _interp.assign_attr(
                self_val.0.clone(),
                "sort_arg_dict",
                Value::dict(PyDict::default()),
            )?;
            _interp.assign_attr(self_val.0.clone(), "sort_type", Value::string(""))?;

            call_method(_interp, self_val.0, "load_stats", &[arg.0])?;
            call_method(_interp, self_val.0, "get_top_level_stats", &[])?;
            Ok(Value::none())
        }

        fn load_stats(#[positional_only] self_val: PyValueRef<'_>, arg: PyValue) -> Result<Value> {
            if arg.0.is_none() {
                _interp.assign_attr(self_val.0.clone(), "stats", Value::dict(PyDict::default()))?;
                return Ok(Value::none());
            }
            if let Ok(path) = PyPath::try_from_value_with_interp(_interp, &arg.0, FN_NAME, "arg") {
                let bytes = std::fs::read(path.as_path()).map_err(|e| {
                    PyError::from_io_error(&e, Some(path.as_path().to_string_lossy().as_ref()))
                })?;
                let loaded = marshal_load_stats(_interp, &bytes)?;
                _interp.assign_attr(self_val.0.clone(), "stats", loaded)?;
                let fname_str = path.as_path().to_string_lossy().into_owned();
                let label = if let Ok(os_mod) = _interp.load_module("os")
                    && let Ok(stat_fn) = _interp.get_attr(&os_mod, "stat")
                    && let Ok(st) = call_pos(_interp, stat_fn, &[Value::string(&fname_str)])
                    && let Ok(mtime) = _interp.get_attr(&st, "st_mtime")
                    && let Ok(time_mod) = _interp.load_module("time")
                    && let Ok(ctime_fn) = _interp.get_attr(&time_mod, "ctime")
                    && let Ok(ct) = call_pos(_interp, ctime_fn, &[mtime])
                    && let Some(cts) = ct.as_str()
                {
                    format!("{cts}    {fname_str}")
                } else {
                    fname_str
                };
                _interp.assign_attr(self_val.0.clone(), "files", Value::list(vec![Value::string(label)]))?;
            } else if _interp.get_attr(&arg.0, "create_stats").is_ok() {
                call_method(_interp, &arg.0, "create_stats", &[])?;
                let prof_stats = _interp.get_attr(&arg.0, "stats")?;
                _interp.assign_attr(self_val.0.clone(), "stats", prof_stats)?;
                let _ = _interp.assign_attr(arg.0.clone(), "stats", Value::dict(PyDict::default()));
            }
            let stats = _interp.get_attr(self_val.0, "stats")?;
            if !_interp.truthy_value(&stats)? {
                let cls_r = render_value_repr(_interp, &value_class(self_val.0))?;
                let arg_r = render_value_repr(_interp, &arg.0)?;
                return Err(PyError::named(
                    "TypeError",
                    format!("Cannot create or construct a {cls_r} object from {arg_r}"),
                ));
            }
            Ok(Value::none())
        }

        fn get_top_level_stats(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let stats = _interp.get_attr(self_val.0, "stats")?;
            let items_val = call_method(_interp, &stats, "items", &[])?;
            let mut total_calls = _interp.get_attr(self_val.0, "total_calls")?.as_int().unwrap_or(0);
            let mut prim_calls = _interp.get_attr(self_val.0, "prim_calls")?.as_int().unwrap_or(0);
            let mut total_tt = val_as_f64(&_interp.get_attr(self_val.0, "total_tt")?).unwrap_or(0.0);
            let mut max_name_len = _interp.get_attr(self_val.0, "max_name_len")?.as_int().unwrap_or(0);
            let top_level = _interp.get_attr(self_val.0, "top_level")?;
            let jprof_key = Value::tuple(vec![
                Value::string("jprofile"),
                Value::int(0),
                Value::string("profiler"),
            ]);

            for pair in _interp.collect_iterable(&items_val)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() != 2 {
                    continue;
                }
                let func = &kv[0];
                let entry = _interp.collect_iterable(&kv[1])?;
                if entry.len() != 5 {
                    continue;
                }
                let cc = entry[0].as_int().unwrap_or(0);
                let nc = entry[1].as_int().unwrap_or(0);
                let tt = PyNumber::try_from_value_with_interp(_interp, &entry[2], FN_NAME, "tt")
                    .map(|n| n.as_f64())
                    .unwrap_or(0.0);
                let callers = &entry[4];
                total_calls += nc;
                prim_calls += cc;
                total_tt += tt;

                let has_jprof = call_method(
                    _interp,
                    callers,
                    "__contains__",
                    std::slice::from_ref(&jprof_key),
                )?;
                if _interp.truthy_value(&has_jprof)? {
                    call_method(_interp, &top_level, "add", std::slice::from_ref(func))?;
                }
                if let Some((f, l, n)) = extract_func_triple(func) {
                    let std_len = func_std_string_rs(&f, l, &n).len() as i64;
                    if std_len > max_name_len {
                        max_name_len = std_len;
                    }
                }
            }
            _interp.assign_attr(self_val.0.clone(), "total_calls", Value::int(total_calls))?;
            _interp.assign_attr(self_val.0.clone(), "prim_calls", Value::int(prim_calls))?;
            _interp.assign_attr(self_val.0.clone(), "total_tt", Value::float(total_tt))?;
            _interp.assign_attr(self_val.0.clone(), "max_name_len", Value::int(max_name_len))?;
            Ok(Value::none())
        }

        fn add(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] arg_list: &[Value],
        ) -> Result<Value> {
            if arg_list.is_empty() {
                return Ok(self_val.0.clone());
            }
            let my_cls = value_class(self_val.0);
            let stats_cls = pstats_attr(_interp, "Stats")?;
            for raw_item in arg_list.iter().rev() {
                let item = if crate::interpreter::values_are_identical(&my_cls, &value_class(raw_item)) {
                    raw_item.clone()
                } else {
                    call_pos(_interp, stats_cls.clone(), std::slice::from_ref(raw_item))?
                };
                let files = _interp.get_attr(self_val.0, "files")?;
                let item_files = _interp.get_attr(&item, "files")?;
                let new_files = _interp.eval_binary(files, crate::ast::BinaryOp::Add, item_files)?;
                _interp.assign_attr(self_val.0.clone(), "files", new_files)?;

                let tc = _interp.get_attr(self_val.0, "total_calls")?.as_int().unwrap_or(0)
                    + _interp.get_attr(&item, "total_calls")?.as_int().unwrap_or(0);
                let pc = _interp.get_attr(self_val.0, "prim_calls")?.as_int().unwrap_or(0)
                    + _interp.get_attr(&item, "prim_calls")?.as_int().unwrap_or(0);
                let tt = val_as_f64(&_interp.get_attr(self_val.0, "total_tt")?).unwrap_or(0.0)
                    + val_as_f64(&_interp.get_attr(&item, "total_tt")?).unwrap_or(0.0);
                _interp.assign_attr(self_val.0.clone(), "total_calls", Value::int(tc))?;
                _interp.assign_attr(self_val.0.clone(), "prim_calls", Value::int(pc))?;
                _interp.assign_attr(self_val.0.clone(), "total_tt", Value::float(tt))?;

                let my_top = _interp.get_attr(self_val.0, "top_level")?;
                let item_top = _interp.get_attr(&item, "top_level")?;
                for f in _interp.collect_iterable(&item_top)? {
                    call_method(_interp, &my_top, "add", &[f])?;
                }

                let my_max = _interp.get_attr(self_val.0, "max_name_len")?.as_int().unwrap_or(0);
                let item_max = _interp.get_attr(&item, "max_name_len")?.as_int().unwrap_or(0);
                if my_max < item_max {
                    _interp.assign_attr(self_val.0.clone(), "max_name_len", Value::int(item_max))?;
                }
                _interp.assign_attr(self_val.0.clone(), "fcn_list", Value::none())?;

                let my_stats = _interp.get_attr(self_val.0, "stats")?;
                let item_stats = _interp.get_attr(&item, "stats")?;
                let items_val = call_method(_interp, &item_stats, "items", &[])?;
                for pair in _interp.collect_iterable(&items_val)? {
                    let kv = _interp.collect_iterable(&pair)?;
                    if kv.len() != 2 {
                        continue;
                    }
                    let func = &kv[0];
                    let stat = &kv[1];
                    let has = call_method(
                        _interp,
                        &my_stats,
                        "__contains__",
                        std::slice::from_ref(func),
                    )?;
                    let old_stat = if _interp.truthy_value(&has)? {
                        _interp.eval_index(&my_stats, func.clone())?
                    } else {
                        Value::tuple(vec![
                            Value::int(0),
                            Value::int(0),
                            Value::float(0.0),
                            Value::float(0.0),
                            Value::dict(PyDict::default()),
                        ])
                    };
                    let merged = add_func_stats_rs(_interp, &old_stat, stat)?;
                    call_method(_interp, &my_stats, "__setitem__", &[func.clone(), merged])?;
                }
            }
            Ok(self_val.0.clone())
        }

        fn dump_stats(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyPath,
        ) -> Result<Value> {
            let stats = _interp.get_attr(self_val.0, "stats")?;
            let bytes = marshal_dump_stats(&stats)?;
            std::fs::write(filename.as_path(), bytes).map_err(|e| {
                PyError::from_io_error(&e, Some(filename.as_path().to_string_lossy().as_ref()))
            })?;
            Ok(Value::none())
        }

        fn get_sort_arg_defs(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let existing = _interp.get_attr(self_val.0, "sort_arg_dict")?;
            if _interp.truthy_value(&existing)? {
                return Ok(existing);
            }
            let expanded = expand_sort_abbrevs();
            let mut d = PyDict::default();
            for (word, def) in expanded {
                let specs: Vec<Value> = def
                    .specs
                    .iter()
                    .map(|(idx, dir)| {
                        Value::tuple(vec![Value::int(*idx as i64), Value::int(i64::from(*dir))])
                    })
                    .collect();
                d.insert(
                    PyKey::str_from(&word),
                    Value::tuple(vec![Value::tuple(specs), Value::string(def.label)]),
                );
            }
            let dict_val = Value::dict(d);
            _interp.assign_attr(self_val.0.clone(), "sort_arg_dict", dict_val.clone())?;
            Ok(dict_val)
        }

        fn sort_stats(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] field: &[Value],
        ) -> Result<Value> {
            if field.is_empty() {
                _interp.assign_attr(self_val.0.clone(), "fcn_list", Value::int(0))?;
                return Ok(self_val.0.clone());
            }
            let words: Vec<String> = if field.len() == 1 && field[0].as_int().is_some() {
                let code = field[0].as_int().unwrap();
                let mapped = match code {
                    -1 => "stdname",
                    0 => "calls",
                    1 => "time",
                    2 => "cumulative",
                    _ => {
                        return Err(PyError::named("KeyError", format!("{code}")));
                    }
                };
                vec![mapped.to_string()]
            } else {
                if field.len() >= 2 {
                    let first_cls = value_class(&field[0]);
                    for arg in &field[1..] {
                        if !crate::interpreter::values_are_identical(&value_class(arg), &first_cls) {
                            return Err(PyError::named("TypeError", "Can't have mixed argument type"));
                        }
                    }
                }
                let sort_key_cls = pstats_attr(_interp, "SortKey").ok();
                let mut out = Vec::with_capacity(field.len());
                for item in field {
                    let is_sk = sort_key_cls
                        .as_ref()
                        .and_then(|cls| {
                            call_pos(
                                _interp,
                                Value::builtin_function("isinstance"),
                                &[item.clone(), cls.clone()],
                            )
                            .ok()
                        })
                        .and_then(|v| _interp.truthy_value(&v).ok())
                        .unwrap_or(false);
                    if is_sk {
                        let v = _interp.get_attr(item, "value")?;
                        out.push(_interp.render_value_as_str(&v)?);
                    } else if let Some(s) = item.as_str() {
                        out.push(s.to_string());
                    } else {
                        return Err(PyError::named("KeyError", render_value_repr(_interp, item)?));
                    }
                }
                out
            };

            let abbrevs = expand_sort_abbrevs();
            let mut specs: Vec<(usize, i8)> = Vec::new();
            let mut sort_labels: Vec<&str> = Vec::new();
            for w in &words {
                let Some(def) = abbrevs.get(w) else {
                    return Err(PyError::named("KeyError", format!("'{w}'")));
                };
                specs.extend_from_slice(&def.specs);
                sort_labels.push(def.label);
            }
            _interp.assign_attr(
                self_val.0.clone(),
                "sort_type",
                Value::string(sort_labels.join(", ")),
            )?;

            struct SortRow {
                cc: i64,
                nc: i64,
                tt: f64,
                ct: f64,
                filename: String,
                lineno: i64,
                name: String,
                stdname: String,
                func: Value,
            }

            let stats = _interp.get_attr(self_val.0, "stats")?;
            let items_val = call_method(_interp, &stats, "items", &[])?;
            let mut rows = Vec::new();
            for pair in _interp.collect_iterable(&items_val)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() != 2 {
                    continue;
                }
                let func = kv[0].clone();
                let entry = _interp.collect_iterable(&kv[1])?;
                if entry.len() != 5 {
                    continue;
                }
                let (filename, lineno, name) =
                    extract_func_triple(&func).unwrap_or_else(|| (String::new(), 0, String::new()));
                let stdname = func_std_string_rs(&filename, lineno, &name);
                let cc = entry[0].as_int().unwrap_or(0);
                let nc = entry[1].as_int().unwrap_or(0);
                let tt = PyNumber::try_from_value_with_interp(_interp, &entry[2], FN_NAME, "tt")
                    .map(|n| n.as_f64())
                    .unwrap_or(0.0);
                let ct = PyNumber::try_from_value_with_interp(_interp, &entry[3], FN_NAME, "ct")
                    .map(|n| n.as_f64())
                    .unwrap_or(0.0);
                rows.push(SortRow {
                    cc,
                    nc,
                    tt,
                    ct,
                    filename,
                    lineno,
                    name,
                    stdname,
                    func,
                });
            }

            rows.sort_by(|a, b| {
                for &(idx, dir) in &specs {
                    let ord = match idx {
                        0 => a.cc.cmp(&b.cc),
                        1 => a.nc.cmp(&b.nc),
                        2 => a.tt.total_cmp(&b.tt),
                        3 => a.ct.total_cmp(&b.ct),
                        4 => a.filename.cmp(&b.filename),
                        5 => a.lineno.cmp(&b.lineno),
                        6 => a.name.cmp(&b.name),
                        7 => a.stdname.cmp(&b.stdname),
                        _ => Ordering::Equal,
                    };
                    if ord != Ordering::Equal {
                        return if dir < 0 { ord.reverse() } else { ord };
                    }
                }
                Ordering::Equal
            });

            let fcn_list: Vec<Value> = rows.into_iter().map(|r| r.func).collect();
            _interp.assign_attr(self_val.0.clone(), "fcn_list", Value::list(fcn_list))?;
            Ok(self_val.0.clone())
        }

        fn reverse_order(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let fcn_list = _interp.get_attr(self_val.0, "fcn_list")?;
            if _interp.truthy_value(&fcn_list)? {
                call_method(_interp, &fcn_list, "reverse", &[])?;
            }
            Ok(self_val.0.clone())
        }

        fn strip_dirs(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let oldstats = _interp.get_attr(self_val.0, "stats")?;
            let newstats = Value::dict(PyDict::default());
            let mut max_name_len = 0i64;
            let items_val = call_method(_interp, &oldstats, "items", &[])?;
            for pair in _interp.collect_iterable(&items_val)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() != 2 {
                    continue;
                }
                let newfunc = func_strip_path_rs(_interp, &kv[0])?;
                if let Some((f, l, n)) = extract_func_triple(&newfunc) {
                    let len = func_std_string_rs(&f, l, &n).len() as i64;
                    if len > max_name_len {
                        max_name_len = len;
                    }
                }
                let entry = _interp.collect_iterable(&kv[1])?;
                if entry.len() != 5 {
                    continue;
                }
                let newcallers = Value::dict(PyDict::default());
                let c_items = call_method(_interp, &entry[4], "items", &[])?;
                for cp in _interp.collect_iterable(&c_items)? {
                    let ckv = _interp.collect_iterable(&cp)?;
                    if ckv.len() == 2 {
                        let stripped = func_strip_path_rs(_interp, &ckv[0])?;
                        call_method(_interp, &newcallers, "__setitem__", &[stripped, ckv[1].clone()])?;
                    }
                }
                let new_entry = Value::tuple(vec![
                    entry[0].clone(),
                    entry[1].clone(),
                    entry[2].clone(),
                    entry[3].clone(),
                    newcallers,
                ]);
                let has = call_method(
                    _interp,
                    &newstats,
                    "__contains__",
                    std::slice::from_ref(&newfunc),
                )?;
                if _interp.truthy_value(&has)? {
                    let prev = _interp.eval_index(&newstats, newfunc.clone())?;
                    let merged = add_func_stats_rs(_interp, &prev, &new_entry)?;
                    call_method(_interp, &newstats, "__setitem__", &[newfunc, merged])?;
                } else {
                    call_method(_interp, &newstats, "__setitem__", &[newfunc, new_entry])?;
                }
            }

            let old_top = _interp.get_attr(self_val.0, "top_level")?;
            let new_top = Value::set(PySet::default());
            for f in _interp.collect_iterable(&old_top)? {
                let stripped = func_strip_path_rs(_interp, &f)?;
                call_method(_interp, &new_top, "add", &[stripped])?;
            }
            _interp.assign_attr(self_val.0.clone(), "stats", newstats)?;
            _interp.assign_attr(self_val.0.clone(), "top_level", new_top)?;
            _interp.assign_attr(self_val.0.clone(), "max_name_len", Value::int(max_name_len))?;
            _interp.assign_attr(self_val.0.clone(), "fcn_list", Value::none())?;
            _interp.assign_attr(self_val.0.clone(), "all_callees", Value::none())?;
            Ok(self_val.0.clone())
        }

        fn calc_callees(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let existing = _interp.get_attr(self_val.0, "all_callees")?;
            if _interp.truthy_value(&existing)? {
                return Ok(Value::none());
            }
            let all_callees = Value::dict(PyDict::default());
            let stats = _interp.get_attr(self_val.0, "stats")?;
            let items_val = call_method(_interp, &stats, "items", &[])?;
            for pair in _interp.collect_iterable(&items_val)? {
                let kv = _interp.collect_iterable(&pair)?;
                if kv.len() != 2 {
                    continue;
                }
                let func = &kv[0];
                let entry = _interp.collect_iterable(&kv[1])?;
                if entry.len() != 5 {
                    continue;
                }
                let has_func = call_method(
                    _interp,
                    &all_callees,
                    "__contains__",
                    std::slice::from_ref(func),
                )?;
                if !_interp.truthy_value(&has_func)? {
                    call_method(
                        _interp,
                        &all_callees,
                        "__setitem__",
                        &[func.clone(), Value::dict(PyDict::default())],
                    )?;
                }
                let c_items = call_method(_interp, &entry[4], "items", &[])?;
                for cp in _interp.collect_iterable(&c_items)? {
                    let ckv = _interp.collect_iterable(&cp)?;
                    if ckv.len() != 2 {
                        continue;
                    }
                    let func2 = &ckv[0];
                    let caller = &ckv[1];
                    let has_f2 = call_method(
                        _interp,
                        &all_callees,
                        "__contains__",
                        std::slice::from_ref(func2),
                    )?;
                    if !_interp.truthy_value(&has_f2)? {
                        call_method(
                            _interp,
                            &all_callees,
                            "__setitem__",
                            &[func2.clone(), Value::dict(PyDict::default())],
                        )?;
                    }
                    let sub = _interp.eval_index(&all_callees, func2.clone())?;
                    call_method(_interp, &sub, "__setitem__", &[func.clone(), caller.clone()])?;
                }
            }
            _interp.assign_attr(self_val.0.clone(), "all_callees", all_callees)?;
            Ok(Value::none())
        }

        fn eval_print_amount(
            #[positional_only] _self_val: PyValueRef<'_>,
            sel: PyValue,
            list: PyValue,
            msg: PyStr<'_>,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(&list.0)?;
            let mut new_list = items.clone();
            let mut out_msg = msg.0.to_string();

            if let Some(pat) = sel.0.as_str() {
                let re_mod = _interp.load_module("re")?;
                let compile_fn = _interp.get_attr(&re_mod, "compile")?;
                match call_pos(_interp, compile_fn, &[Value::string(pat)]) {
                    Ok(rex) => {
                        let search_fn = _interp.get_attr(&rex, "search")?;
                        new_list = Vec::new();
                        for func in &items {
                            let std_name = match extract_func_triple(func) {
                                Some((f, l, n)) => func_std_string_rs(&f, l, &n),
                                None => _interp.render_value_as_str(func)?,
                            };
                            let matched =
                                call_pos(_interp, search_fn.clone(), &[Value::string(std_name)])?;
                            if _interp.truthy_value(&matched)? {
                                new_list.push(func.clone());
                            }
                        }
                    }
                    Err(_) => {
                        let r = render_value_repr(_interp, &sel.0)?;
                        out_msg.push_str(&format!("   <Invalid regular expression {r}>\n"));
                        return Ok(Value::tuple(vec![
                            Value::list(new_list),
                            Value::string(out_msg),
                        ]));
                    }
                }
            } else if let ValueKind::Float(frac) = sel.0.kind() {
                if (0.0..1.0).contains(&frac) {
                    let count = ((items.len() as f64) * frac + 0.5) as usize;
                    new_list = items[..count.min(items.len())].to_vec();
                }
            } else if let Some(n) = sel.0.as_int()
                && n >= 0
                && (n as usize) < items.len()
            {
                new_list = items[..n as usize].to_vec();
            }

            if items.len() != new_list.len() {
                let r = render_value_repr(_interp, &sel.0)?;
                out_msg.push_str(&format!(
                    "   List reduced from {} to {} due to restriction <{r}>\n",
                    items.len(),
                    new_list.len()
                ));
            }
            Ok(Value::tuple(vec![
                Value::list(new_list),
                Value::string(out_msg),
            ]))
        }

        fn get_stats_profile(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let fcn_list = _interp.get_attr(self_val.0, "fcn_list")?;
            let stats = _interp.get_attr(self_val.0, "stats")?;
            let func_list = if _interp.truthy_value(&fcn_list)? {
                _interp.collect_iterable(&fcn_list)?
            } else {
                let keys = call_method(_interp, &stats, "keys", &[])?;
                _interp.collect_iterable(&keys)?
            };
            let sp_cls = pstats_attr(_interp, "StatsProfile")?;
            let fp_cls = pstats_attr(_interp, "FunctionProfile")?;
            if func_list.is_empty() {
                return call_pos(
                    _interp,
                    sp_cls,
                    &[Value::int(0), Value::dict(PyDict::default())],
                );
            }
            let total_tt_raw = val_as_f64(&_interp.get_attr(self_val.0, "total_tt")?).unwrap_or(0.0);
            let total_tt: f64 = f8_str(total_tt_raw).trim().parse().unwrap_or(total_tt_raw);
            let func_profiles = Value::dict(PyDict::default());

            for func in func_list {
                let entry_val = _interp.eval_index(&stats, func.clone())?;
                let entry = _interp.collect_iterable(&entry_val)?;
                if entry.len() != 5 {
                    continue;
                }
                let cc = entry[0].as_int().unwrap_or(0);
                let nc = entry[1].as_int().unwrap_or(0);
                let tt = PyNumber::try_from_value_with_interp(_interp, &entry[2], FN_NAME, "tt")
                    .map(|n| n.as_f64())
                    .unwrap_or(0.0);
                let ct = PyNumber::try_from_value_with_interp(_interp, &entry[3], FN_NAME, "ct")
                    .map(|n| n.as_f64())
                    .unwrap_or(0.0);
                let (file_name, line_number, func_name) =
                    extract_func_triple(&func).unwrap_or_else(|| (String::new(), 0, String::new()));
                let ncalls = if nc == cc {
                    format!("{nc}")
                } else {
                    format!("{nc}/{cc}")
                };
                let tottime: f64 = f8_str(tt).trim().parse().unwrap_or(tt);
                let percall_tottime = if nc == 0 {
                    Value::int(-1)
                } else {
                    let v = tt / nc as f64;
                    Value::float(f8_str(v).trim().parse().unwrap_or(v))
                };
                let cumtime: f64 = f8_str(ct).trim().parse().unwrap_or(ct);
                let percall_cumtime = if cc == 0 {
                    Value::int(-1)
                } else {
                    let v = ct / cc as f64;
                    Value::float(f8_str(v).trim().parse().unwrap_or(v))
                };
                let fp = call_pos(
                    _interp,
                    fp_cls.clone(),
                    &[
                        Value::string(ncalls),
                        Value::float(tottime),
                        percall_tottime,
                        Value::float(cumtime),
                        percall_cumtime,
                        Value::string(file_name),
                        Value::int(line_number),
                    ],
                )?;
                call_method(
                    _interp,
                    &func_profiles,
                    "__setitem__",
                    &[Value::string(func_name), fp],
                )?;
            }
            call_pos(_interp, sp_cls, &[Value::float(total_tt), func_profiles])
        }

        fn get_print_list(
            #[positional_only] self_val: PyValueRef<'_>,
            sel_list: PyValue,
        ) -> Result<Value> {
            let mut width = _interp.get_attr(self_val.0, "max_name_len")?.as_int().unwrap_or(0);
            let fcn_list = _interp.get_attr(self_val.0, "fcn_list")?;
            let stats = _interp.get_attr(self_val.0, "stats")?;
            let (mut stat_list, mut msg) = if _interp.truthy_value(&fcn_list)? {
                let st = _interp.get_attr(self_val.0, "sort_type")?;
                let st_s = _interp.render_value_as_str(&st)?;
                (
                    Value::list(_interp.collect_iterable(&fcn_list)?),
                    format!("   Ordered by: {st_s}\n"),
                )
            } else {
                let keys = call_method(_interp, &stats, "keys", &[])?;
                (
                    Value::list(_interp.collect_iterable(&keys)?),
                    "   Random listing order was used\n".to_string(),
                )
            };

            for selection in _interp.collect_iterable(&sel_list.0)? {
                let res = call_method(
                    _interp,
                    self_val.0,
                    "eval_print_amount",
                    &[selection, stat_list, Value::string(msg)],
                )?;
                let pair = _interp.collect_iterable(&res)?;
                stat_list = pair[0].clone();
                msg = _interp.render_value_as_str(&pair[1])?;
            }

            let items = _interp.collect_iterable(&stat_list)?;
            if items.is_empty() {
                return Ok(Value::tuple(vec![Value::int(0), stat_list]));
            }
            let stream = _interp.get_attr(self_val.0, "stream")?;
            write_stream(_interp, &stream, &format!("{msg}\n"))?;
            let stats_keys = call_method(_interp, &stats, "keys", &[])?;
            let total_stats = _interp.collect_iterable(&stats_keys)?.len();
            if items.len() < total_stats {
                width = 0;
                for func in &items {
                    if let Some((f, l, n)) = extract_func_triple(func) {
                        let len = func_std_string_rs(&f, l, &n).len() as i64;
                        if len > width {
                            width = len;
                        }
                    }
                }
            }
            Ok(Value::tuple(vec![Value::int(width + 2), stat_list]))
        }

        fn print_stats(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] amount: &[Value],
        ) -> Result<Value> {
            let stream = _interp.get_attr(self_val.0, "stream")?;
            let files = _interp.get_attr(self_val.0, "files")?;
            let file_list = _interp.collect_iterable(&files)?;
            for f in &file_list {
                let fs = _interp.render_value_as_str(f)?;
                write_stream(_interp, &stream, &format!("{fs}\n"))?;
            }
            if !file_list.is_empty() {
                write_stream(_interp, &stream, "\n")?;
            }
            let indent = "        ";
            let top_level = _interp.get_attr(self_val.0, "top_level")?;
            for func in _interp.collect_iterable(&top_level)? {
                let fname = _interp.eval_index(&func, Value::int(2))?;
                let fs = _interp.render_value_as_str(&fname)?;
                write_stream(_interp, &stream, &format!("{indent} {fs}\n"))?;
            }
            let total_calls = _interp.get_attr(self_val.0, "total_calls")?.as_int().unwrap_or(0);
            let prim_calls = _interp.get_attr(self_val.0, "prim_calls")?.as_int().unwrap_or(0);
            let total_tt = val_as_f64(&_interp.get_attr(self_val.0, "total_tt")?).unwrap_or(0.0);
            if total_calls != prim_calls {
                write_stream(
                    _interp,
                    &stream,
                    &format!(
                        "{indent} {total_calls} function calls ({prim_calls} primitive calls) in {total_tt:.3} seconds\n\n"
                    ),
                )?;
            } else {
                write_stream(
                    _interp,
                    &stream,
                    &format!("{indent} {total_calls} function calls in {total_tt:.3} seconds\n\n"),
                )?;
            }

            let pair = call_method(
                _interp,
                self_val.0,
                "get_print_list",
                &[Value::tuple(amount.to_vec())],
            )?;
            let parts = _interp.collect_iterable(&pair)?;
            let func_list = _interp.collect_iterable(&parts[1])?;
            if !func_list.is_empty() {
                call_method(_interp, self_val.0, "print_title", &[])?;
                for func in func_list {
                    call_method(_interp, self_val.0, "print_line", &[func])?;
                }
                write_stream(_interp, &stream, "\n\n")?;
            }
            Ok(self_val.0.clone())
        }

        fn print_callees(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] amount: &[Value],
        ) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "get_print_list",
                &[Value::tuple(amount.to_vec())],
            )?;
            let parts = _interp.collect_iterable(&pair)?;
            let width = parts[0].clone();
            let func_list = _interp.collect_iterable(&parts[1])?;
            if !func_list.is_empty() {
                call_method(_interp, self_val.0, "calc_callees", &[])?;
                call_method(
                    _interp,
                    self_val.0,
                    "print_call_heading",
                    &[width.clone(), Value::string("called...")],
                )?;
                let all_callees = _interp.get_attr(self_val.0, "all_callees")?;
                for func in func_list {
                    let has = call_method(
                        _interp,
                        &all_callees,
                        "__contains__",
                        std::slice::from_ref(&func),
                    )?;
                    let callees = if _interp.truthy_value(&has)? {
                        _interp.eval_index(&all_callees, func.clone())?
                    } else {
                        Value::dict(PyDict::default())
                    };
                    call_method(
                        _interp,
                        self_val.0,
                        "print_call_line",
                        &[width.clone(), func, callees],
                    )?;
                }
                let stream = _interp.get_attr(self_val.0, "stream")?;
                write_stream(_interp, &stream, "\n\n")?;
            }
            Ok(self_val.0.clone())
        }

        fn print_callers(
            #[positional_only] self_val: PyValueRef<'_>,
            #[varargs] amount: &[Value],
        ) -> Result<Value> {
            let pair = call_method(
                _interp,
                self_val.0,
                "get_print_list",
                &[Value::tuple(amount.to_vec())],
            )?;
            let parts = _interp.collect_iterable(&pair)?;
            let width = parts[0].clone();
            let func_list = _interp.collect_iterable(&parts[1])?;
            if !func_list.is_empty() {
                call_method(
                    _interp,
                    self_val.0,
                    "print_call_heading",
                    &[width.clone(), Value::string("was called by...")],
                )?;
                let stats = _interp.get_attr(self_val.0, "stats")?;
                for func in func_list {
                    let entry = _interp.eval_index(&stats, func.clone())?;
                    let callers = _interp.eval_index(&entry, Value::int(4))?;
                    call_method(
                        _interp,
                        self_val.0,
                        "print_call_line",
                        &[width.clone(), func, callers, Value::string("<-")],
                    )?;
                }
                let stream = _interp.get_attr(self_val.0, "stream")?;
                write_stream(_interp, &stream, "\n\n")?;
            }
            Ok(self_val.0.clone())
        }

        fn print_call_heading(
            #[positional_only] self_val: PyValueRef<'_>,
            name_size: PyValue,
            column_title: PyStr<'_>,
        ) -> Result<Value> {
            let n = name_size.0.as_int().unwrap_or(0).max(0) as usize;
            let stream = _interp.get_attr(self_val.0, "stream")?;
            let left = format!("{:<width$}", "Function ", width = n);
            write_stream(_interp, &stream, &format!("{left}{}\n", column_title.0))?;
            let stats = _interp.get_attr(self_val.0, "stats")?;
            let vals = call_method(_interp, &stats, "values", &[])?;
            let mut subheader = false;
            for entry in _interp.collect_iterable(&vals)? {
                let parts = _interp.collect_iterable(&entry)?;
                if parts.len() == 5 && _interp.truthy_value(&parts[4])? {
                    let cvals = call_method(_interp, &parts[4], "values", &[])?;
                    if let Some(first) = _interp.collect_iterable(&cvals)?.first() {
                        subheader = first.as_tuple().is_some();
                    }
                    break;
                }
            }
            if subheader {
                let pad = " ".repeat(n);
                write_stream(_interp, &stream, &format!("{pad}    ncalls  tottime  cumtime\n"))?;
            }
            Ok(Value::none())
        }

        fn print_call_line(
            #[positional_only] self_val: PyValueRef<'_>,
            name_size: PyValue,
            source: PyValue,
            call_dict: PyValue,
            #[default(OptionalArg::Omitted)] arrow: OptValueRef<'_>,
        ) -> Result<Value> {
            let n = name_size.0.as_int().unwrap_or(0).max(0) as usize;
            let arrow_str = arrow
                .as_value()
                .and_then(|v| v.as_str())
                .unwrap_or("->");
            let stream = _interp.get_attr(self_val.0, "stream")?;
            let src_name = match extract_func_triple(&source.0) {
                Some((f, l, nm)) => func_std_string_rs(&f, l, &nm),
                None => _interp.render_value_as_str(&source.0)?,
            };
            let padded = format!("{src_name:<width$}", width = n);
            write_stream(_interp, &stream, &format!("{padded}{arrow_str} "))?;
            if !_interp.truthy_value(&call_dict.0)? {
                write_stream(_interp, &stream, "\n")?;
                return Ok(Value::none());
            }
            let keys_val = call_method(_interp, &call_dict.0, "keys", &[])?;
            let sorted_keys = call_pos(_interp, Value::builtin_function("sorted"), &[keys_val])?;
            let stats = _interp.get_attr(self_val.0, "stats")?;
            let mut indent_len = 0usize;
            for func in _interp.collect_iterable(&sorted_keys)? {
                let name = match extract_func_triple(&func) {
                    Some((f, l, nm)) => func_std_string_rs(&f, l, &nm),
                    None => _interp.render_value_as_str(&func)?,
                };
                let value = _interp.eval_index(&call_dict.0, func.clone())?;
                let (substats, left_width) = if let Some(t) = value.as_tuple()
                    && t.len() == 4
                {
                    let nc = t[0].as_int().unwrap_or(0);
                    let cc = t[1].as_int().unwrap_or(0);
                    let tt = PyNumber::try_from_value_with_interp(_interp, &t[2], FN_NAME, "tt")
                        .map(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let ct = PyNumber::try_from_value_with_interp(_interp, &t[3], FN_NAME, "ct")
                        .map(|v| v.as_f64())
                        .unwrap_or(0.0);
                    let count_str = if nc != cc {
                        format!("{nc}/{cc}")
                    } else {
                        format!("{nc}")
                    };
                    let rj = 7 + 2 * indent_len;
                    (
                        format!("{count_str:>rj$} {} {}  {name}", f8_str(tt), f8_str(ct)),
                        n + 1,
                    )
                } else {
                    let vr = render_value_repr(_interp, &value)?;
                    let ct = _interp
                        .eval_index(&stats, func)
                        .and_then(|e| _interp.eval_index(&e, Value::int(3)))
                        .and_then(|v| {
                            PyNumber::try_from_value_with_interp(_interp, &v, FN_NAME, "ct")
                                .map(|n| n.as_f64())
                        })
                        .unwrap_or(0.0);
                    (format!("{name}({vr}) {}", f8_str(ct)), n + 3)
                };
                let pad = if indent_len > 0 {
                    " ".repeat(left_width)
                } else {
                    String::new()
                };
                write_stream(_interp, &stream, &format!("{pad}{substats}\n"))?;
                indent_len = 1;
            }
            Ok(Value::none())
        }

        fn print_title(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let stream = _interp.get_attr(self_val.0, "stream")?;
            write_stream(
                _interp,
                &stream,
                "   ncalls  tottime  percall  cumtime  percall filename:lineno(function)\n",
            )?;
            Ok(Value::none())
        }

        fn print_line(#[positional_only] self_val: PyValueRef<'_>, func: PyValue) -> Result<Value> {
            let stream = _interp.get_attr(self_val.0, "stream")?;
            let stats = _interp.get_attr(self_val.0, "stats")?;
            let entry_val = _interp.eval_index(&stats, func.0.clone())?;
            let entry = _interp.collect_iterable(&entry_val)?;
            let cc = entry[0].as_int().unwrap_or(0);
            let nc = entry[1].as_int().unwrap_or(0);
            let tt = PyNumber::try_from_value_with_interp(_interp, &entry[2], FN_NAME, "tt")
                .map(|n| n.as_f64())
                .unwrap_or(0.0);
            let ct = PyNumber::try_from_value_with_interp(_interp, &entry[3], FN_NAME, "ct")
                .map(|n| n.as_f64())
                .unwrap_or(0.0);
            let c_str = if nc != cc {
                format!("{nc}/{cc}")
            } else {
                format!("{nc}")
            };
            let per_tt = if nc == 0 {
                "        ".to_string()
            } else {
                f8_str(tt / nc as f64)
            };
            let per_ct = if cc == 0 {
                "        ".to_string()
            } else {
                f8_str(ct / cc as f64)
            };
            let std_name = match extract_func_triple(&func.0) {
                Some((f, l, n)) => func_std_string_rs(&f, l, &n),
                None => _interp.render_value_as_str(&func.0)?,
            };
            write_stream(
                _interp,
                &stream,
                &format!(
                    "{c_str:>9} {} {per_tt} {} {per_ct} {std_name}\n",
                    f8_str(tt),
                    f8_str(ct)
                ),
            )?;
            Ok(Value::none())
        }
    }
}
