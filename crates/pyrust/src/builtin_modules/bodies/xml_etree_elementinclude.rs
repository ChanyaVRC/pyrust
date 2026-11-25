// `xml.etree.ElementInclude` module — 100% native-Rust Python 3.12 XInclude
// processor for ElementTree (issue #3260).
//
// Provides `XINCLUDE`, `XINCLUDE_INCLUDE`, `XINCLUDE_FALLBACK`,
// `DEFAULT_MAX_INCLUSION_DEPTH`, `FatalIncludeError`,
// `LimitedRecursiveIncludeError`, `default_loader`, and `include`.
//
// Reference: <https://docs.python.org/3/library/xml.etree.elementinclude.html>

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{OptValueRef, OptionalArg, PyStr, PyValue};
use crate::interpreter::{ExpandedCallArg, Interpreter};
use crate::value::{PyModule, Value};

const XINCLUDE_NS: &str = "{http://www.w3.org/2001/XInclude}";
const XINCLUDE_INCLUDE_TAG: &str = "{http://www.w3.org/2001/XInclude}include";
const XINCLUDE_FALLBACK_TAG: &str = "{http://www.w3.org/2001/XInclude}fallback";
const DEFAULT_MAX_DEPTH: i64 = 6;

fn call_pos(interp: &mut Interpreter, callee: Value, args: &[Value]) -> Result<Value> {
    let expanded: Vec<ExpandedCallArg> = args
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    interp.call_function_expanded(callee, &expanded)
}

fn call_kw(
    interp: &mut Interpreter,
    callee: Value,
    pos: &[Value],
    kw: &[(&str, Value)],
) -> Result<Value> {
    let mut expanded: Vec<ExpandedCallArg> = pos
        .iter()
        .cloned()
        .map(|value| ExpandedCallArg { name: None, value })
        .collect();
    for (k, v) in kw {
        expanded.push(ExpandedCallArg {
            name: Some((*k).to_string()),
            value: v.clone(),
        });
    }
    interp.call_function_expanded(callee, &expanded)
}

fn call_method(interp: &mut Interpreter, obj: &Value, name: &str, args: &[Value]) -> Result<Value> {
    let m = interp.get_attr(obj, name)?;
    call_pos(interp, m, args)
}

fn raise_ei_exc(interp: &mut Interpreter, cls_name: &'static str, msg: String) -> PyError {
    if let Ok(m) = interp.load_module("xml.etree.ElementInclude")
        && let Ok(cls) = interp.get_attr(&m, cls_name)
        && let Ok(inst) = call_pos(interp, cls, &[Value::string(&msg)])
    {
        return PyError::raised(inst);
    }
    PyError::named(cls_name, msg)
}

fn default_loader_impl(
    interp: &mut Interpreter,
    href: &str,
    parse: &str,
    encoding: Option<&str>,
) -> Result<Value> {
    if parse == "xml" {
        let fp = call_pos(
            interp,
            Value::builtin_function("open"),
            &[Value::string(href), Value::string("rb")],
        )?;
        let et_mod = interp.load_module("xml.etree.ElementTree")?;
        let parse_fn = interp.get_attr(&et_mod, "parse")?;
        let tree_res = call_pos(interp, parse_fn, std::slice::from_ref(&fp));
        let _ = call_method(interp, &fp, "close", &[]);
        let tree = tree_res?;
        call_method(interp, &tree, "getroot", &[])
    } else {
        let enc = encoding.unwrap_or("utf-8");
        let fp = call_kw(
            interp,
            Value::builtin_function("open"),
            &[Value::string(href), Value::string("r")],
            &[("encoding", Value::string(enc))],
        )?;
        let text_res = call_method(interp, &fp, "read", &[]);
        let _ = call_method(interp, &fp, "close", &[]);
        text_res
    }
}

fn join_base_href(interp: &mut Interpreter, base: &str, href: &str) -> Result<String> {
    if base.is_empty() || href.contains("://") || href.starts_with('/') {
        return Ok(href.to_string());
    }
    if base.contains("://") {
        let up = interp.load_module("urllib.parse")?;
        let uj = interp.get_attr(&up, "urljoin")?;
        let joined = call_pos(interp, uj, &[Value::string(base), Value::string(href)])?;
        return Ok(joined.as_str().unwrap_or(href).to_string());
    }
    if let Some(slash) = base.rfind('/') {
        Ok(format!("{}{href}", &base[..=slash]))
    } else {
        Ok(href.to_string())
    }
}

fn include_recursive(
    interp: &mut Interpreter,
    elem: &Value,
    loader: Option<&Value>,
    base_url: Option<&str>,
    max_depth: Option<i64>,
    parent_hrefs: &mut HashSet<String>,
) -> Result<()> {
    let mut i: usize = 0;
    loop {
        let cur_len = call_pos(
            interp,
            Value::builtin_function("len"),
            std::slice::from_ref(elem),
        )?
        .as_int()
        .unwrap_or(0) as usize;
        if i >= cur_len {
            break;
        }
        let child = call_method(interp, elem, "__getitem__", &[Value::int(i as i64)])?;
        let tag_v = interp.get_attr(&child, "tag")?;
        let tag = tag_v.as_str().unwrap_or("");

        if tag == XINCLUDE_INCLUDE_TAG {
            let href_v = call_method(interp, &child, "get", &[Value::string("href")])?;
            let raw_href = href_v.as_str().unwrap_or("").to_string();
            let href = if let Some(base) = base_url {
                join_base_href(interp, base, &raw_href)?
            } else {
                raw_href
            };

            let parse_v = call_method(
                interp,
                &child,
                "get",
                &[Value::string("parse"), Value::string("xml")],
            )?;
            let parse_mode = parse_v.as_str().unwrap_or("xml").to_string();

            if parse_mode == "xml" {
                if parent_hrefs.contains(&href) {
                    return Err(raise_ei_exc(
                        interp,
                        "FatalIncludeError",
                        format!("recursive include of {href}"),
                    ));
                }
                if let Some(md) = max_depth
                    && md == 0
                {
                    return Err(raise_ei_exc(
                        interp,
                        "LimitedRecursiveIncludeError",
                        format!("maximum xinclude depth reached when including file {href}"),
                    ));
                }
                parent_hrefs.insert(href.clone());
                let loaded = if let Some(ldr) = loader {
                    call_pos(
                        interp,
                        ldr.clone(),
                        &[Value::string(&href), Value::string(&parse_mode)],
                    )
                } else {
                    default_loader_impl(interp, &href, &parse_mode, None)
                };
                let node = match loaded {
                    Ok(v) if !v.is_none() => v,
                    Ok(_) => {
                        parent_hrefs.remove(&href);
                        return Err(raise_ei_exc(
                            interp,
                            "FatalIncludeError",
                            format!("cannot load {href:?} as {parse_mode:?}"),
                        ));
                    }
                    Err(e) => {
                        parent_hrefs.remove(&href);
                        return Err(e);
                    }
                };
                let copy_mod = interp.load_module("copy")?;
                let copy_fn = interp.get_attr(&copy_mod, "copy")?;
                let node_copy = call_pos(interp, copy_fn, &[node])?;
                let next_depth = max_depth.map(|d| d - 1);
                let rec_res = include_recursive(
                    interp,
                    &node_copy,
                    loader,
                    Some(&href),
                    next_depth,
                    parent_hrefs,
                );
                parent_hrefs.remove(&href);
                rec_res?;

                let child_tail = interp.get_attr(&child, "tail")?;
                if !child_tail.is_none() {
                    let existing_tail = interp.get_attr(&node_copy, "tail")?;
                    let new_tail = if let Some(et) = existing_tail.as_str() {
                        format!("{et}{}", child_tail.as_str().unwrap_or(""))
                    } else {
                        child_tail.as_str().unwrap_or("").to_string()
                    };
                    interp.assign_attr(node_copy.clone(), "tail", Value::string(new_tail))?;
                }
                call_method(
                    interp,
                    elem,
                    "__setitem__",
                    &[Value::int(i as i64), node_copy],
                )?;
            } else if parse_mode == "text" {
                let enc_v = call_method(interp, &child, "get", &[Value::string("encoding")])?;
                let enc_opt = enc_v.as_str();
                let txt_val = if let Some(ldr) = loader {
                    call_pos(
                        interp,
                        ldr.clone(),
                        &[
                            Value::string(&href),
                            Value::string(&parse_mode),
                            enc_v.clone(),
                        ],
                    )?
                } else {
                    default_loader_impl(interp, &href, &parse_mode, enc_opt)?
                };
                if txt_val.is_none() {
                    return Err(raise_ei_exc(
                        interp,
                        "FatalIncludeError",
                        format!("cannot load {href:?} as {parse_mode:?}"),
                    ));
                }
                let txt = txt_val.as_str().unwrap_or("");
                let child_tail = interp.get_attr(&child, "tail")?;
                let full = if let Some(ct) = child_tail.as_str() {
                    format!("{txt}{ct}")
                } else {
                    txt.to_string()
                };
                if i > 0 {
                    let prev =
                        call_method(interp, elem, "__getitem__", &[Value::int((i - 1) as i64)])?;
                    let pt = interp.get_attr(&prev, "tail")?;
                    let merged = format!("{}{full}", pt.as_str().unwrap_or(""));
                    interp.assign_attr(prev, "tail", Value::string(merged))?;
                } else {
                    let et = interp.get_attr(elem, "text")?;
                    let merged = format!("{}{full}", et.as_str().unwrap_or(""));
                    interp.assign_attr(elem.clone(), "text", Value::string(merged))?;
                }
                call_method(interp, elem, "__delitem__", &[Value::int(i as i64)])?;
                continue;
            } else {
                return Err(raise_ei_exc(
                    interp,
                    "FatalIncludeError",
                    format!("unknown parse type in xi:include tag ({parse_mode:?})"),
                ));
            }
        } else if tag == XINCLUDE_FALLBACK_TAG {
            return Err(raise_ei_exc(
                interp,
                "FatalIncludeError",
                format!("xi:fallback tag must be child of xi:include ({tag:?})"),
            ));
        } else {
            include_recursive(interp, &child, loader, base_url, max_depth, parent_hrefs)?;
        }
        i += 1;
    }
    Ok(())
}

pub(crate) fn post_load_init(
    interp: &mut Interpreter,
    module: &Rc<RefCell<PyModule>>,
) -> Result<()> {
    let et_mod = interp.load_module("xml.etree.ElementTree")?;
    let copy_mod = interp.load_module("copy")?;
    let up_mod = interp.load_module("urllib.parse")?;
    let urljoin_fn = interp.get_attr(&up_mod, "urljoin")?;
    let mut m = module.borrow_mut();
    m.insert_attr("ElementTree".to_string(), et_mod);
    m.insert_attr("copy".to_string(), copy_mod);
    m.insert_attr("urljoin".to_string(), urljoin_fn);
    Ok(())
}

pyrust_module! {
    constants {
        "__package__" => Value::string("xml.etree"),
        "XINCLUDE" => Value::string(XINCLUDE_NS),
        "XINCLUDE_INCLUDE" => Value::string(XINCLUDE_INCLUDE_TAG),
        "XINCLUDE_FALLBACK" => Value::string(XINCLUDE_FALLBACK_TAG),
        "DEFAULT_MAX_INCLUSION_DEPTH" => Value::int(DEFAULT_MAX_DEPTH),
    }

    exception FatalIncludeError(SyntaxError);
    exception LimitedRecursiveIncludeError(FatalIncludeError);

    fn default_loader(
        href: PyStr<'_>,
        parse: PyStr<'_>,
        #[default(OptionalArg::Omitted)] encoding: OptValueRef<'_>,
    ) -> Result<Value> {
        let enc = encoding.as_value().and_then(|v| v.as_str());
        default_loader_impl(_interp, &href.0, &parse.0, enc)
    }

    fn include(
        elem: PyValue,
        #[default(OptionalArg::Omitted)] loader: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] base_url: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] max_depth: OptValueRef<'_>,
    ) -> Result<Value> {
        let md: Option<i64> = match max_depth.as_value() {
            None => Some(DEFAULT_MAX_DEPTH),
            Some(v) if v.is_none() => None,
            Some(v) => {
                let Some(n) = v.as_int() else {
                    return Err(PyError::named(
                        "TypeError",
                        "expected 'max_depth' to be a non-negative integer or None",
                    ));
                };
                if n < 0 {
                    return Err(PyError::named(
                        "ValueError",
                        format!("expected non-negative depth or None for 'max_depth', got {n}"),
                    ));
                }
                Some(n)
            }
        };
        let ldr = loader.as_value().filter(|v| !v.is_none());
        let base = base_url.as_value().and_then(|v| v.as_str());
        let mut parent_hrefs = HashSet::new();
        include_recursive(_interp, &elem.0, ldr, base, md, &mut parent_hrefs)?;
        Ok(Value::none())
    }
}
