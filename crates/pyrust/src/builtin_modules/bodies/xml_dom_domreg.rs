// `xml.dom.domreg` module — 100% native-Rust Python 3.12 DOM implementation
// registry: `registered`, `well_known_implementations`,
// `registerDOMImplementation`, `getDOMImplementation` (issue #3260).
//
// Reference: <https://github.com/python/cpython/blob/3.12/Lib/xml/dom/domreg.py>

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyStr, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyDict, PyKey, Value};

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(interp: &mut Interpreter, obj: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let m = interp.get_attr(obj, name)?;
    call_pos(interp, m, args)
}

fn build_well_known() -> Value {
    let mut d = PyDict::default();
    d.insert(PyKey::str_from("minidom"), Value::string("xml.dom.minidom"));
    d.insert(
        PyKey::str_from("4DOM"),
        Value::string("xml.dom.DOMImplementation"),
    );
    Value::dict(d)
}

fn parse_feature_string(s: &str) -> Result<Vec<(String, Option<String>)>> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    let mut features = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        let feature = parts[i];
        if feature.starts_with(|c: char| c.is_ascii_digit()) {
            return Err(PyError::named(
                "ValueError",
                format!("bad feature name: {feature:?}"),
            ));
        }
        i += 1;
        let version = if i < parts.len() && parts[i].starts_with(|c: char| c.is_ascii_digit()) {
            let v = parts[i].to_string();
            i += 1;
            Some(v)
        } else {
            None
        };
        features.push((feature.to_string(), version));
    }
    Ok(features)
}

fn good_enough(
    interp: &mut Interpreter,
    dom: &Value,
    features: &[(String, Option<String>)],
) -> Result<bool> {
    for (f, v) in features {
        let ver_val = match v {
            Some(vs) => Value::string(vs),
            None => Value::none(),
        };
        let has = call_method(interp, dom, "hasFeature", &[Value::string(f), ver_val])?;
        if !interp.truthy_value(&has)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn get_impl_from_module(interp: &mut Interpreter, mod_name: &str) -> Result<Value> {
    let m = interp.load_module(mod_name)?;
    let f = interp.get_attr(&m, "getDOMImplementation")?;
    call_pos(interp, f, &[])
}

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.dom"),
        "well_known_implementations" => build_well_known(),
        "registered" => Value::dict(PyDict::default()),
    }

    #[py_name = "registerDOMImplementation"]
    fn register_dom_implementation(name: PyStr<'_>, factory: PyValue) -> Result<Value> {
        let m = _interp.load_module("xml.dom.domreg")?;
        let reg = _interp.get_attr(&m, "registered")?;
        call_method(
            _interp,
            &reg,
            "__setitem__",
            &[Value::string(name.0.as_ref()), factory.0],
        )?;
        Ok(Value::none())
    }

    #[py_name = "getDOMImplementation"]
    fn get_dom_implementation(
        #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] features: OptValueRef<'_>,
    ) -> Result<Value> {
        let m = _interp.load_module("xml.dom.domreg")?;
        let reg = _interp.get_attr(&m, "registered")?;
        let well_known = _interp.get_attr(&m, "well_known_implementations")?;

        if let Some(name_val) = name.as_value()
            && !name_val.is_none()
        {
            let name_s = name_val
                .as_str()
                .ok_or_else(|| PyError::named("TypeError", "name must be str or None"))?;
            let factory = call_method(_interp, &reg, "get", &[Value::string(name_s), Value::none()])?;
            if !factory.is_none() {
                return call_pos(_interp, factory, &[]);
            }
            let mapped = call_method(
                _interp,
                &well_known,
                "get",
                &[Value::string(name_s), Value::string(name_s)],
            )?;
            let mod_name = mapped.as_str().unwrap_or(name_s);
            return get_impl_from_module(_interp, mod_name);
        }

        let feat_list: Vec<(String, Option<String>)> = match features.as_value() {
            None => Vec::new(),
            Some(v) if v.is_none() => Vec::new(),
            Some(v) => {
                if let Some(s) = v.as_str() {
                    parse_feature_string(s)?
                } else {
                    let mut out = Vec::new();
                    for item in _interp.collect_iterable(v)? {
                        let pair = _interp.collect_iterable(&item)?;
                        if pair.len() != 2 {
                            return Err(PyError::named("ValueError", "feature must be a 2-tuple"));
                        }
                        let f = pair[0]
                            .as_str()
                            .ok_or_else(|| PyError::named("TypeError", "feature name must be str"))?
                            .to_string();
                        let ver = if pair[1].is_none() {
                            None
                        } else {
                            Some(
                                pair[1]
                                    .as_str()
                                    .ok_or_else(|| {
                                        PyError::named("TypeError", "feature version must be str or None")
                                    })?
                                    .to_string(),
                            )
                        };
                        out.push((f, ver));
                    }
                    out
                }
            }
        };

        // Check registered factories first
        let reg_vals = call_method(_interp, &reg, "values", &[])?;
        for factory in _interp.collect_iterable(&reg_vals)? {
            if let Ok(dom) = call_pos(_interp, factory, &[])
                && good_enough(_interp, &dom, &feat_list).unwrap_or(false)
            {
                return Ok(dom);
            }
        }

        // Check well-known implementations
        let wk_vals = call_method(_interp, &well_known, "values", &[])?;
        for mod_val in _interp.collect_iterable(&wk_vals)? {
            if let Some(mod_name) = mod_val.as_str()
                && let Ok(dom) = get_impl_from_module(_interp, mod_name)
                && good_enough(_interp, &dom, &feat_list).unwrap_or(false)
            {
                return Ok(dom);
            }
        }

        Err(PyError::named(
            "ImportError",
            "no suitable DOM implementation found",
        ))
    }
}
