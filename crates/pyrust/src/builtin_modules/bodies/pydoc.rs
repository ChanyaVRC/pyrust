// `pydoc` module — 100% native Rust implementation of Python 3.12's `pydoc`
// documentation generator and interactive `help()` engine (issue #3255).
//
// Reference: <https://docs.python.org/3/library/pydoc.html>

use std::rc::Rc;

use indexmap::IndexMap;
use pyrust_derive::pyrust_module;

use crate::error::{PyError, Result};
use crate::interpreter::builtin_args::{
    OptValueRef, OptionalArg, PyIndex, PyStr, PyValue, PyValueRef,
};
use crate::interpreter::{
    ExpandedCallArg, Interpreter, lookup_exc_class, object_class_singleton,
    primitive_class_by_name, render_instance_str, render_value_repr, type_class_singleton,
    value_class, value_type_name_str, values_are_identical,
};
use crate::value::{PyDict, PyKey, Value, ValueKind};

include!("pydoc/tables.rs");
include!("pydoc/helpers.rs");
include!("pydoc/renderers.rs");

struct PagerWrapperState {
    kind: String,
    cmd: String,
    use_plain: bool,
}

/// Wire cross-module bases (`TextRepr`/`HTMLRepr` -> `reprlib.Repr`),
/// method aliases (`Doc.doc* = Doc.fail`, `docproperty = docdata`),
/// `Doc.PYTHONDOCS`, `TextDoc`/`HTMLDoc` repr instances, `Helper._GoInteractive`,
/// and module-level singletons (`text`, `plaintext`, `html`, `help`).
pub(crate) fn prepare_module(interp: &mut Interpreter, module: &Value) {
    let ValueKind::PyModule(mod_rc) = module.kind() else {
        return;
    };

    // 1. Link TextRepr and HTMLRepr to reprlib.Repr
    if let Ok(reprlib_mod) = interp.load_module("reprlib")
        && let Ok(repr_val) = interp.get_attr(&reprlib_mod, "Repr")
        && let ValueKind::PyClass(repr_cls) = repr_val.kind()
    {
        for name in ["TextRepr", "HTMLRepr"] {
            if let Some(cls_val) = mod_rc.borrow().attrs.get(name).cloned()
                && let ValueKind::PyClass(cls_rc) = cls_val.kind()
            {
                crate::builtin_modules::macros::link_class_bases(cls_rc, vec![Rc::clone(repr_cls)]);
            }
        }
    }

    // 2. Wire Doc.PYTHONDOCS and Doc.doc* = Doc.fail
    if let Some(doc_val) = mod_rc.borrow().attrs.get("Doc").cloned()
        && let ValueKind::PyClass(doc_cls) = doc_val.kind()
    {
        let pydocs_url = interp
            .load_module("sys")
            .and_then(|s| interp.get_attr(&s, "version_info"))
            .and_then(|vi| interp.collect_iterable(&vi))
            .ok()
            .and_then(|parts| {
                let maj = parts.first()?.as_int()?;
                let min = parts.get(1)?.as_int()?;
                Some(format!("https://docs.python.org/{maj}.{min}/library"))
            })
            .unwrap_or_else(|| "https://docs.python.org/3.12/library".to_string());
        let pythondocs = interp
            .load_module("os")
            .and_then(|o| interp.get_attr(&o, "environ"))
            .and_then(|env| {
                call_method(
                    interp,
                    &env,
                    "get",
                    &[Value::string("PYTHONDOCS"), Value::string(&pydocs_url)],
                )
            })
            .unwrap_or_else(|_| Value::string(pydocs_url));
        let fail_fn = doc_cls.borrow().attrs.get("fail").cloned();
        let mut dc = doc_cls.borrow_mut();
        dc.attrs.insert("PYTHONDOCS".to_string(), pythondocs);
        if let Some(f) = fail_fn {
            for alias in [
                "docmodule",
                "docclass",
                "docroutine",
                "docother",
                "docproperty",
                "docdata",
            ] {
                dc.attrs.insert(alias.to_string(), f.clone());
            }
        }
    }

    // 3. Wire TextDoc._repr_instance, TextDoc.repr, TextDoc.docproperty = TextDoc.docdata
    if let Some(td_val) = mod_rc.borrow().attrs.get("TextDoc").cloned()
        && let ValueKind::PyClass(td_cls) = td_val.kind()
    {
        let tr_cls = mod_rc.borrow().attrs.get("TextRepr").cloned();
        if let Some(tr_cls) = tr_cls
            && let Ok(tr_inst) = call_pos(interp, tr_cls, &[])
            && let Ok(tr_repr) = interp.get_attr(&tr_inst, "repr")
        {
            let mut td = td_cls.borrow_mut();
            td.attrs.insert("_repr_instance".to_string(), tr_inst);
            td.attrs.insert("repr".to_string(), tr_repr);
        }
        let docdata = td_cls.borrow().attrs.get("docdata").cloned();
        if let Some(dd) = docdata {
            td_cls
                .borrow_mut()
                .attrs
                .insert("docproperty".to_string(), dd);
        }
    }

    // 4. Wire HTMLDoc._repr_instance, HTMLDoc.repr, HTMLDoc.escape, HTMLDoc.docproperty = HTMLDoc.docdata
    if let Some(hd_val) = mod_rc.borrow().attrs.get("HTMLDoc").cloned()
        && let ValueKind::PyClass(hd_cls) = hd_val.kind()
    {
        let hr_cls = mod_rc.borrow().attrs.get("HTMLRepr").cloned();
        if let Some(hr_cls) = hr_cls
            && let Ok(hr_inst) = call_pos(interp, hr_cls, &[])
            && let Ok(hr_repr) = interp.get_attr(&hr_inst, "repr")
            && let Ok(hr_escape) = interp.get_attr(&hr_inst, "escape")
        {
            let mut hd = hd_cls.borrow_mut();
            hd.attrs.insert("_repr_instance".to_string(), hr_inst);
            hd.attrs.insert("repr".to_string(), hr_repr);
            hd.attrs.insert("escape".to_string(), hr_escape);
        }
        let docdata = hd_cls.borrow().attrs.get("docdata").cloned();
        if let Some(dd) = docdata {
            hd_cls
                .borrow_mut()
                .attrs
                .insert("docproperty".to_string(), dd);
        }
    }

    // 5. Wire Helper._GoInteractive sentinel
    if let Some(h_val) = mod_rc.borrow().attrs.get("Helper").cloned()
        && let ValueKind::PyClass(h_cls) = h_val.kind()
    {
        let obj_cls = Value::py_class(object_class_singleton());
        if let Ok(sentinel) = call_pos(interp, obj_cls, &[]) {
            h_cls
                .borrow_mut()
                .attrs
                .insert("_GoInteractive".to_string(), sentinel);
        }
    }

    // 6. Install module-level singletons: text, plaintext, html, help
    let text_cls = mod_rc.borrow().attrs.get("TextDoc").cloned();
    let plain_cls = mod_rc.borrow().attrs.get("_PlainTextDoc").cloned();
    let html_cls = mod_rc.borrow().attrs.get("HTMLDoc").cloned();
    let helper_cls = mod_rc.borrow().attrs.get("Helper").cloned();

    if let Some(c) = text_cls
        && let Ok(inst) = call_pos(interp, c, &[])
    {
        mod_rc.borrow_mut().insert_attr("text".to_string(), inst);
    }
    if let Some(c) = plain_cls
        && let Ok(inst) = call_pos(interp, c, &[])
    {
        mod_rc
            .borrow_mut()
            .insert_attr("plaintext".to_string(), inst);
    }
    if let Some(c) = html_cls
        && let Ok(inst) = call_pos(interp, c, &[])
    {
        mod_rc.borrow_mut().insert_attr("html".to_string(), inst);
    }
    if let Some(c) = helper_cls
        && let Ok(inst) = call_pos(interp, c, &[])
    {
        mod_rc.borrow_mut().insert_attr("help".to_string(), inst);
    }
}

pyrust_module! {
    constants {
        "__doc__" => Value::string(MODULE_DOC),
        "__all__" => Value::list(vec![Value::string("help")]),
        "__author__" => Value::string("Ka-Ping Yee <ping@lfw.org>"),
        "__date__" => Value::string("26 February 2001"),
        "__credits__" => Value::string(MODULE_CREDITS),
    }

    // ── Exception ────────────────────────────────────────────────────────────

    class ErrorDuringImport("Exception") {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            filename: PyValue,
            exc_info: PyValue,
        ) -> Result<Value> {
            if !is_tuple_val(_interp, &exc_info.0) {
                if !py_isinstance_exc_name(_interp, &exc_info.0, "BaseException") {
                    return Err(PyError::named("AssertionError", ""));
                }
                let exc_cls = value_class(&exc_info.0);
                let tb = py_getattr_opt(_interp, &exc_info.0, "__traceback__")
                    .unwrap_or_else(Value::none);
                _interp.assign_attr(self_val.0.clone(), "exc", exc_cls)?;
                _interp.assign_attr(self_val.0.clone(), "value", exc_info.0)?;
                _interp.assign_attr(self_val.0.clone(), "tb", tb)?;
            } else {
                if let Ok(w_mod) = _interp.load_module("warnings")
                    && let Ok(warn_fn) = _interp.get_attr(&w_mod, "warn")
                {
                    let dep_cls = lookup_exc_class("DeprecationWarning")
                        .map(Value::py_class)
                        .unwrap_or_else(Value::none);
                    let _ = call_pos(
                        _interp,
                        warn_fn,
                        &[
                            Value::string(
                                "A tuple value for exc_info is deprecated, use an exception instance",
                            ),
                            dep_cls,
                        ],
                    );
                }
                let parts = _interp.collect_iterable(&exc_info.0)?;
                if parts.len() != 3 {
                    return Err(PyError::named(
                        "ValueError",
                        format!("expected 3 values to unpack, got {}", parts.len()),
                    ));
                }
                _interp.assign_attr(self_val.0.clone(), "exc", parts[0].clone())?;
                _interp.assign_attr(self_val.0.clone(), "value", parts[1].clone())?;
                _interp.assign_attr(self_val.0.clone(), "tb", parts[2].clone())?;
            }
            _interp.assign_attr(self_val.0.clone(), "filename", filename.0)?;
            Ok(Value::none())
        }

        fn __str__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let exc = _interp.get_attr(self_val.0, "exc")?;
            let exc_name = attr_str(_interp, &exc, "__name__")?;
            let filename = attr_str(_interp, self_val.0, "filename")?;
            let value = attr_str(_interp, self_val.0, "value")?;
            Ok(Value::string(format!(
                "problem in {filename} - {exc_name}: {value}"
            )))
        }
    }

    // ── Common helper functions ──────────────────────────────────────────────

    fn pathdirs() -> Result<Value> {
        pathdirs_impl(_interp)
    }

    fn _getowndoc(obj: PyValue) -> Result<Value> {
        Ok(getowndoc_impl(_interp, &obj.0).unwrap_or_else(Value::none))
    }

    fn _getdoc(object: PyValue) -> Result<Value> {
        Ok(match getdoc_raw_impl(_interp, &object.0)? {
            Some(s) => Value::string(s),
            None => Value::none(),
        })
    }

    fn getdoc(object: PyValue) -> Result<Value> {
        Ok(Value::string(getdoc_impl(_interp, &object.0)?))
    }

    fn splitdoc(doc: PyStr<'_>) -> Result<Value> {
        let (synop, rest) = splitdoc_rs(&doc);
        Ok(Value::tuple(vec![Value::string(synop), Value::string(rest)]))
    }

    fn classname(object: PyValue, modname: PyValue) -> Result<Value> {
        Ok(Value::string(classname_impl(_interp, &object.0, &modname.0)?))
    }

    fn parentname(object: PyValue, modname: PyValue) -> Result<Value> {
        parentname_impl(_interp, &object.0, &modname.0)
    }

    fn isdata(object: PyValue) -> Result<Value> {
        Ok(Value::bool_(isdata_impl(_interp, &object.0)?))
    }

    fn replace(text: PyStr<'_>, #[varargs] pairs: &[Value]) -> Result<Value> {
        Ok(Value::string(replace_pairs_rs(
            text.to_string(),
            pairs,
            _interp,
        )?))
    }

    fn cram(text: PyStr<'_>, maxlen: PyIndex) -> Result<Value> {
        let max_n = maxlen.expect_isize("cram", "maxlen")?;
        Ok(Value::string(cram_rs(&text, max_n)))
    }

    fn stripid(text: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(stripid_rs(&text)))
    }

    fn allmethods(cl: PyValue) -> Result<Value> {
        allmethods_impl(_interp, &cl.0)
    }

    fn visiblename(
        name: PyStr<'_>,
        #[default(OptionalArg::Omitted)] all: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] obj: OptValueRef<'_>,
    ) -> Result<Value> {
        visiblename_impl(_interp, &name, all.as_value(), obj.as_value())
    }

    fn classify_class_attrs(object: PyValue) -> Result<Value> {
        let items = classify_class_attrs_impl(_interp, &object.0)?;
        Ok(Value::list(
            items
                .into_iter()
                .map(|(n, k, c, v)| Value::tuple(vec![Value::string(n), Value::string(k), c, v]))
                .collect(),
        ))
    }

    fn sort_attributes(attrs: PyValue, object: PyValue) -> Result<Value> {
        let mut rows: Vec<(String, Value)> = Vec::new();
        for item in _interp.collect_iterable(&attrs.0)? {
            let parts = _interp.collect_iterable(&item)?;
            let name = parts
                .first()
                .map(|v| py_str(_interp, v))
                .transpose()?
                .unwrap_or_default();
            rows.push((name, item));
        }
        let mut field_order: indexmap::IndexMap<String, isize> = indexmap::IndexMap::new();
        if let Some(fields_val) = py_getattr_opt(_interp, &object.0, "_fields")
            && let Ok(fields) = _interp.collect_iterable(&fields_val)
        {
            let len = fields.len() as isize;
            for (i, f) in fields.iter().enumerate() {
                if let Some(s) = f.as_str() {
                    field_order.insert(s.to_string(), (i as isize) - len);
                }
            }
        }
        rows.sort_by(|a, b| {
            let oa = field_order.get(&a.0).copied().unwrap_or(0);
            let ob = field_order.get(&b.0).copied().unwrap_or(0);
            (oa, a.0.as_str()).cmp(&(ob, b.0.as_str()))
        });
        let sorted = Value::list(rows.into_iter().map(|(_, v)| v).collect());
        call_method(_interp, &attrs.0, "clear", &[])?;
        call_method(_interp, &attrs.0, "extend", &[sorted])?;
        Ok(Value::none())
    }

    fn ispackage(path: PyValue) -> Result<Value> {
        Ok(Value::bool_(ispackage_impl(_interp, &path.0)?))
    }

    fn source_synopsis(file: PyValue) -> Result<Value> {
        source_synopsis_impl(_interp, &file.0)
    }

    fn synopsis(
        filename: PyValue,
        #[default(OptionalArg::Omitted)] cache: OptValueRef<'_>,
    ) -> Result<Value> {
        synopsis_impl(_interp, &filename.0, cache.as_value())
    }

    fn importfile(path: PyValue) -> Result<Value> {
        importfile_impl(_interp, &path.0)
    }

    fn safeimport(
        path: PyStr<'_>,
        #[default(OptionalArg::Omitted)] forceload: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] cache: OptValueRef<'_>,
    ) -> Result<Value> {
        let force = match forceload.as_value() {
            Some(v) => _interp.truthy_value(v)?,
            None => false,
        };
        safeimport_impl(_interp, &path, force, cache.as_value())
    }

    fn describe(thing: PyValue) -> Result<Value> {
        Ok(Value::string(describe_impl(_interp, &thing.0)?))
    }

    fn locate(
        path: PyStr<'_>,
        #[default(OptionalArg::Omitted)] forceload: OptValueRef<'_>,
    ) -> Result<Value> {
        let force = match forceload.as_value() {
            Some(v) => _interp.truthy_value(v)?,
            None => false,
        };
        locate_impl(_interp, &path, force)
    }

    fn resolve(
        thing: PyValue,
        #[default(OptionalArg::Omitted)] forceload: OptValueRef<'_>,
    ) -> Result<Value> {
        let force = match forceload.as_value() {
            Some(v) => _interp.truthy_value(v)?,
            None => false,
        };
        let (obj, name) = resolve_impl(_interp, &thing.0, force)?;
        Ok(Value::tuple(vec![obj, name]))
    }

    fn plain(text: PyStr<'_>) -> Result<Value> {
        Ok(Value::string(plain_rs(&text)))
    }

    fn pager(text: PyStr<'_>) -> Result<Value> {
        let getpager_fn = pydoc_attr(_interp, "getpager")?;
        let chosen = call_pos(_interp, getpager_fn, &[])?;
        if let Ok(pydoc_mod) = _interp.load_module("pydoc") {
            let _ = _interp.assign_attr(pydoc_mod, "pager", chosen.clone());
        }
        call_pos(_interp, chosen, &[Value::string(&*text)])
    }

    fn getpager() -> Result<Value> {
        getpager_impl(_interp)
    }

    fn plainpager(text: PyStr<'_>) -> Result<Value> {
        plainpager_impl(_interp, &text)?;
        Ok(Value::none())
    }

    fn pipepager(text: PyStr<'_>, cmd: PyStr<'_>) -> Result<Value> {
        pipepager_impl(_interp, &text, &cmd)?;
        Ok(Value::none())
    }

    fn tempfilepager(text: PyStr<'_>, cmd: PyStr<'_>) -> Result<Value> {
        tempfilepager_impl(_interp, &text, &cmd)?;
        Ok(Value::none())
    }

    fn ttypager(text: PyStr<'_>) -> Result<Value> {
        ttypager_impl(_interp, &text)?;
        Ok(Value::none())
    }

    fn render_doc(
        thing: PyValue,
        #[default(OptionalArg::Omitted)] title: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] forceload: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] renderer: OptValueRef<'_>,
    ) -> Result<Value> {
        let title_v = title
            .as_value()
            .cloned()
            .unwrap_or_else(|| Value::string("Python Library Documentation: %s"));
        let force = match forceload.as_value() {
            Some(v) => _interp.truthy_value(v)?,
            None => false,
        };
        let rendered = render_doc_impl(
            _interp,
            &thing.0,
            &title_v,
            force,
            renderer.as_value().cloned(),
        )?;
        Ok(Value::string(rendered))
    }

    fn doc(
        thing: PyValue,
        #[default(OptionalArg::Omitted)] title: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] forceload: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] output: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] is_cli: OptValueRef<'_>,
    ) -> Result<Value> {
        let title_v = title
            .as_value()
            .cloned()
            .unwrap_or_else(|| Value::string("Python Library Documentation: %s"));
        let force = match forceload.as_value() {
            Some(v) => _interp.truthy_value(v)?,
            None => false,
        };
        let cli_flag = match is_cli.as_value() {
            Some(v) => _interp.truthy_value(v)?,
            None => false,
        };
        doc_impl(
            _interp,
            &thing.0,
            &title_v,
            force,
            output.as_value().cloned(),
            cli_flag,
        )?;
        Ok(Value::none())
    }

    fn writedoc(
        thing: PyValue,
        #[default(OptionalArg::Omitted)] forceload: OptValueRef<'_>,
    ) -> Result<Value> {
        let force = match forceload.as_value() {
            Some(v) => _interp.truthy_value(v)?,
            None => false,
        };
        writedoc_impl(_interp, &thing.0, force)?;
        Ok(Value::none())
    }

    fn writedocs(
        dir: PyValue,
        #[default(OptionalArg::Omitted)] pkgpath: OptValueRef<'_>,
        #[default(OptionalArg::Omitted)] done: OptValueRef<'_>,
    ) -> Result<Value> {
        let pkg = match pkgpath.as_value() {
            Some(v) => py_str(_interp, v)?,
            None => String::new(),
        };
        writedocs_impl(_interp, &dir.0, &pkg, done.as_value().cloned())?;
        Ok(Value::none())
    }

    fn apropos(key: PyStr<'_>) -> Result<Value> {
        apropos_impl(_interp, &key)?;
        Ok(Value::none())
    }

    fn _apropos_callback(_path: PyValue, modname: PyStr<'_>, desc: PyStr<'_>) -> Result<Value> {
        let display_name = match modname.strip_suffix(".__init__") {
            Some(prefix) => format!("{prefix} (package)"),
            None => modname.to_string(),
        };
        let tail = if desc.is_empty() {
            String::new()
        } else {
            format!("- {}", &*desc)
        };
        py_print(_interp, &[Value::string(display_name), Value::string(tail)])?;
        Ok(Value::none())
    }

    fn _apropos_onerror(_modname: PyValue) -> Result<Value> {
        Ok(Value::none())
    }

    fn ispath(x: PyValue) -> Result<Value> {
        Ok(Value::bool_(ispath_impl(_interp, &x.0)?))
    }

    fn _get_revised_path(given_path: PyValue, argv0: PyValue) -> Result<Value> {
        get_revised_path_impl(_interp, &given_path.0, &argv0.0)
    }

    fn _adjust_cli_sys_path() -> Result<Value> {
        adjust_cli_sys_path_impl(_interp)?;
        Ok(Value::none())
    }

    fn cli() -> Result<Value> {
        cli_impl(_interp)?;
        Ok(Value::none())
    }

    // ── Internal callable for getpager() wrappers ────────────────────────────

    #[native_state(PagerWrapperState)]
    class _PagerWrapper {
        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            kind: PyStr<'_>,
            cmd: PyStr<'_>,
            use_plain: PyValue,
        ) -> Result<Value> {
            let up = _interp.truthy_value(&use_plain.0)?;
            set_state(
                self_val.0,
                PagerWrapperState {
                    kind: kind.to_string(),
                    cmd: cmd.to_string(),
                    use_plain: up,
                },
            )?;
            Ok(Value::none())
        }

        fn __call__(#[positional_only] self_val: PyValueRef<'_>, text: PyStr<'_>) -> Result<Value> {
            let (kind, cmd, use_plain) =
                with_state(self_val.0, |st| Ok((st.kind.clone(), st.cmd.clone(), st.use_plain)))?;
            let payload = if use_plain {
                plain_rs(&text)
            } else {
                text.to_string()
            };
            if kind == "tempfile" {
                tempfilepager_impl(_interp, &payload, &cmd)?;
            } else {
                pipepager_impl(_interp, &payload, &cmd)?;
            }
            Ok(Value::none())
        }
    }

    // ── Doc base class ───────────────────────────────────────────────────────

    class Doc {
        fn document(
            #[positional_only] self_val: PyValueRef<'_>,
            object: PyValue,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let name_v = name.as_value().cloned().unwrap_or_else(Value::none);
            doc_document_impl(_interp, self_val.0, &object.0, name_v, args)
        }

        fn fail(
            #[positional_only] _self: PyValueRef<'_>,
            object: PyValue,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[varargs] _args: &[Value],
        ) -> Result<Value> {
            doc_fail_impl(_interp, &object.0, name.as_value())
        }

        fn getdocloc(
            #[positional_only] self_val: PyValueRef<'_>,
            object: PyValue,
            #[default(OptionalArg::Omitted)] basedir: OptValueRef<'_>,
        ) -> Result<Value> {
            doc_getdocloc_impl(_interp, self_val.0, &object.0, basedir.as_value().cloned())
        }
    }

    // ── TextRepr & HTMLRepr (bases linked to reprlib.Repr in prepare_module) ─

    class TextRepr {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let reprlib = _interp.load_module("reprlib")?;
            let repr_cls = _interp.get_attr(&reprlib, "Repr")?;
            let repr_init = _interp.get_attr(&repr_cls, "__init__")?;
            call_pos(_interp, repr_init, std::slice::from_ref(self_val.0))?;
            _interp.assign_attr(self_val.0.clone(), "maxlist", Value::int(20))?;
            _interp.assign_attr(self_val.0.clone(), "maxtuple", Value::int(20))?;
            _interp.assign_attr(self_val.0.clone(), "maxdict", Value::int(10))?;
            _interp.assign_attr(self_val.0.clone(), "maxstring", Value::int(100))?;
            _interp.assign_attr(self_val.0.clone(), "maxother", Value::int(100))?;
            Ok(Value::none())
        }

        fn repr1(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            level: PyValue,
        ) -> Result<Value> {
            let t = value_class(&x.0);
            if let Ok(tname_v) = _interp.get_attr(&t, "__name__")
                && let Some(tname) = tname_v.as_str()
            {
                let mname = format!(
                    "repr_{}",
                    tname.split_whitespace().collect::<Vec<_>>().join("_")
                );
                if let Ok(method) = _interp.get_attr(self_val.0, &mname) {
                    return call_pos(_interp, method, &[x.0, level.0]);
                }
            }
            let maxother = _interp
                .get_attr(self_val.0, "maxother")
                .ok()
                .and_then(|v| v.as_int())
                .unwrap_or(100) as isize;
            Ok(Value::string(cram_rs(&stripid_rs(&py_repr(_interp, &x.0)?), maxother)))
        }

        fn repr_string(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyStr<'_>,
            _level: PyValue,
        ) -> Result<Value> {
            let maxstring = _interp
                .get_attr(self_val.0, "maxstring")
                .ok()
                .and_then(|v| v.as_int())
                .unwrap_or(100) as isize;
            Ok(Value::string(text_repr_string_rs(_interp, &x, maxstring)?))
        }

        fn repr_str(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyStr<'_>,
            level: PyValue,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "repr_string", &[Value::string(&*x), level.0])
        }

        fn repr_instance(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            _level: PyValue,
        ) -> Result<Value> {
            let maxstring = _interp
                .get_attr(self_val.0, "maxstring")
                .ok()
                .and_then(|v| v.as_int())
                .unwrap_or(100) as isize;
            match py_repr(_interp, &x.0) {
                Ok(r) => Ok(Value::string(cram_rs(&stripid_rs(&r), maxstring))),
                Err(_) => {
                    let cname = value_type_name_str(&x.0);
                    Ok(Value::string(format!("<{cname} instance>")))
                }
            }
        }
    }

    class HTMLRepr {
        fn __init__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let reprlib = _interp.load_module("reprlib")?;
            let repr_cls = _interp.get_attr(&reprlib, "Repr")?;
            let repr_init = _interp.get_attr(&repr_cls, "__init__")?;
            call_pos(_interp, repr_init, std::slice::from_ref(self_val.0))?;
            _interp.assign_attr(self_val.0.clone(), "maxlist", Value::int(20))?;
            _interp.assign_attr(self_val.0.clone(), "maxtuple", Value::int(20))?;
            _interp.assign_attr(self_val.0.clone(), "maxdict", Value::int(10))?;
            _interp.assign_attr(self_val.0.clone(), "maxstring", Value::int(100))?;
            _interp.assign_attr(self_val.0.clone(), "maxother", Value::int(100))?;
            Ok(Value::none())
        }

        fn escape(#[positional_only] _self: PyValueRef<'_>, text: PyStr<'_>) -> Result<Value> {
            Ok(Value::string(html_escape_rs(&text)))
        }

        fn repr(#[positional_only] self_val: PyValueRef<'_>, object: PyValue) -> Result<Value> {
            let reprlib = _interp.load_module("reprlib")?;
            let repr_cls = _interp.get_attr(&reprlib, "Repr")?;
            let repr_fn = _interp.get_attr(&repr_cls, "repr")?;
            call_pos(_interp, repr_fn, &[self_val.0.clone(), object.0])
        }

        fn repr1(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            level: PyValue,
        ) -> Result<Value> {
            let t = value_class(&x.0);
            if let Ok(tname_v) = _interp.get_attr(&t, "__name__")
                && let Some(tname) = tname_v.as_str()
            {
                let mname = format!(
                    "repr_{}",
                    tname.split_whitespace().collect::<Vec<_>>().join("_")
                );
                if let Ok(method) = _interp.get_attr(self_val.0, &mname) {
                    return call_pos(_interp, method, &[x.0, level.0]);
                }
            }
            let maxother = _interp
                .get_attr(self_val.0, "maxother")
                .ok()
                .and_then(|v| v.as_int())
                .unwrap_or(100) as isize;
            let text = cram_rs(&stripid_rs(&py_repr(_interp, &x.0)?), maxother);
            call_method(_interp, self_val.0, "escape", &[Value::string(text)])
        }

        fn repr_string(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyStr<'_>,
            _level: PyValue,
        ) -> Result<Value> {
            let maxstring = _interp
                .get_attr(self_val.0, "maxstring")
                .ok()
                .and_then(|v| v.as_int())
                .unwrap_or(100) as isize;
            Ok(Value::string(html_repr_string_rs(_interp, &x, maxstring)?))
        }

        fn repr_str(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyStr<'_>,
            level: PyValue,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "repr_string", &[Value::string(&*x), level.0])
        }

        fn repr_unicode(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyStr<'_>,
            level: PyValue,
        ) -> Result<Value> {
            call_method(_interp, self_val.0, "repr_string", &[Value::string(&*x), level.0])
        }

        fn repr_instance(
            #[positional_only] self_val: PyValueRef<'_>,
            x: PyValue,
            _level: PyValue,
        ) -> Result<Value> {
            let maxstring = _interp
                .get_attr(self_val.0, "maxstring")
                .ok()
                .and_then(|v| v.as_int())
                .unwrap_or(100) as isize;
            let text = match py_repr(_interp, &x.0) {
                Ok(r) => cram_rs(&stripid_rs(&r), maxstring),
                Err(_) => format!("<{} instance>", value_type_name_str(&x.0)),
            };
            call_method(_interp, self_val.0, "escape", &[Value::string(text)])
        }
    }

    // ── TextDoc & _PlainTextDoc ──────────────────────────────────────────────

    class TextDoc("Doc") {
        fn bold(#[positional_only] _self: PyValueRef<'_>, text: PyStr<'_>) -> Result<Value> {
            Ok(Value::string(bold_rs(&text)))
        }

        fn indent(
            #[positional_only] _self: PyValueRef<'_>,
            text: PyStr<'_>,
            #[default(OptionalArg::Omitted)] prefix: OptValueRef<'_>,
        ) -> Result<Value> {
            let pref = match prefix.as_value() {
                Some(v) => py_str(_interp, v)?,
                None => "    ".to_string(),
            };
            Ok(Value::string(indent_rs(&text, &pref)))
        }

        fn section(
            #[positional_only] self_val: PyValueRef<'_>,
            title: PyStr<'_>,
            contents: PyStr<'_>,
        ) -> Result<Value> {
            let indented =
                call_method_str(_interp, self_val.0, "indent", &[Value::string(&*contents)])?;
            let bold_title =
                call_method_str(_interp, self_val.0, "bold", &[Value::string(&*title)])?;
            Ok(Value::string(format!(
                "{bold_title}\n{}\n\n",
                indented.trim_end()
            )))
        }

        fn formattree(
            #[positional_only] self_val: PyValueRef<'_>,
            tree: PyValue,
            modname: PyValue,
            #[default(OptionalArg::Omitted)] parent: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] prefix: OptValueRef<'_>,
        ) -> Result<Value> {
            let parent_v = parent.as_value().cloned().unwrap_or_else(Value::none);
            let pref = match prefix.as_value() {
                Some(v) => py_str(_interp, v)?,
                None => String::new(),
            };
            Ok(Value::string(textdoc_formattree_impl(
                _interp,
                self_val.0,
                &tree.0,
                &modname.0,
                &parent_v,
                &pref,
            )?))
        }

        fn docmodule(
            #[positional_only] self_val: PyValueRef<'_>,
            object: PyValue,
            #[default(OptionalArg::Omitted)] _name: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _mod: OptValueRef<'_>,
            #[varargs] _ignored: &[Value],
        ) -> Result<Value> {
            Ok(Value::string(textdoc_docmodule_impl(
                _interp, self_val.0, &object.0,
            )?))
        }

        fn docclass(
            #[positional_only] self_val: PyValueRef<'_>,
            object: PyValue,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] mod_val: OptValueRef<'_>,
            #[varargs] _ignored: &[Value],
        ) -> Result<Value> {
            let mv = mod_val.as_value().cloned().unwrap_or_else(Value::none);
            Ok(Value::string(textdoc_docclass_impl(
                _interp,
                self_val.0,
                &object.0,
                name.as_value(),
                mv,
            )?))
        }

        fn formatvalue(
            #[positional_only] self_val: PyValueRef<'_>,
            object: PyValue,
        ) -> Result<Value> {
            let r = call_method_str(_interp, self_val.0, "repr", &[object.0])?;
            Ok(Value::string(format!("={r}")))
        }

        fn docroutine(
            #[positional_only] self_val: PyValueRef<'_>,
            object: PyValue,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] mod_val: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] cl: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] homecls: OptValueRef<'_>,
        ) -> Result<Value> {
            let mv = mod_val.as_value().cloned().unwrap_or_else(Value::none);
            Ok(Value::string(textdoc_docroutine_impl(
                _interp,
                self_val.0,
                &object.0,
                name.as_value(),
                &mv,
                cl.as_value(),
                homecls.as_value(),
            )?))
        }

        fn docdata(
            #[positional_only] self_val: PyValueRef<'_>,
            object: PyValue,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _mod: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _cl: OptValueRef<'_>,
            #[varargs] _ignored: &[Value],
        ) -> Result<Value> {
            Ok(Value::string(textdoc_docdata_impl(
                _interp,
                self_val.0,
                &object.0,
                name.as_value(),
            )?))
        }

        fn docother(
            #[positional_only] self_val: PyValueRef<'_>,
            object: PyValue,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _mod: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _parent: OptValueRef<'_>,
            #[varargs] _ignored: &[Value],
            #[default(OptionalArg::Omitted)] maxlen: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] doc: OptValueRef<'_>,
        ) -> Result<Value> {
            let max_n = match maxlen.as_value() {
                Some(v) if !v.is_none() => v.as_int().filter(|&n| n > 0).map(|n| n as usize),
                _ => None,
            };
            Ok(Value::string(textdoc_docother_impl(
                _interp,
                self_val.0,
                &object.0,
                name.as_value(),
                max_n,
                doc.as_value().cloned(),
            )?))
        }
    }

    class _PlainTextDoc("TextDoc") {
        fn bold(#[positional_only] _self: PyValueRef<'_>, text: PyStr<'_>) -> Result<Value> {
            Ok(Value::string(&*text))
        }
    }

    // ── HTMLDoc ──────────────────────────────────────────────────────────────

    class HTMLDoc("Doc") {
        fn page(
            #[positional_only] _self: PyValueRef<'_>,
            title: PyStr<'_>,
            contents: PyStr<'_>,
        ) -> Result<Value> {
            let title_s: &str = &title;
            let contents_s: &str = &contents;
            Ok(Value::string(format!(
                "<!DOCTYPE html>\n\
<html lang=\"en\">\n\
<head>\n\
<meta charset=\"utf-8\">\n\
<title>Python: {title_s}</title>\n\
</head><body>\n\
{contents_s}\n\
</body></html>"
            )))
        }

        fn heading(
            #[positional_only] _self: PyValueRef<'_>,
            title: PyStr<'_>,
            #[default(OptionalArg::Omitted)] extras: OptValueRef<'_>,
        ) -> Result<Value> {
            let title_s: &str = &title;
            let ex = match extras.as_value() {
                Some(v) if _interp.truthy_value(v)? => py_str(_interp, v)?,
                _ => "&nbsp;".to_string(),
            };
            Ok(Value::string(format!(
                "\n<table class=\"heading\">\n\
<tr class=\"heading-text decor\">\n\
<td class=\"title\">&nbsp;<br>{title_s}</td>\n\
<td class=\"extra\">{ex}</td></tr></table>\n    "
            )))
        }

        fn section(
            #[positional_only] _self: PyValueRef<'_>,
            title: PyStr<'_>,
            cls: PyStr<'_>,
            contents: PyStr<'_>,
            #[default(OptionalArg::Omitted)] width: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] prelude: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] marginalia: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] gap: OptValueRef<'_>,
        ) -> Result<Value> {
            let w = match width.as_value() {
                Some(v) => v.as_int().unwrap_or(6).max(0) as usize,
                None => 6,
            };
            let pre = match prelude.as_value() {
                Some(v) if !v.is_none() => py_str(_interp, v)?,
                _ => String::new(),
            };
            let marg = match marginalia.as_value() {
                Some(v) if !v.is_none() => Some(py_str(_interp, v)?),
                _ => None,
            };
            let g = match gap.as_value() {
                Some(v) => py_str(_interp, v)?,
                None => "&nbsp;".to_string(),
            };
            Ok(Value::string(htmldoc_section_impl(
                &title,
                &cls,
                &contents,
                w,
                &pre,
                marg.as_deref(),
                &g,
            )))
        }

        fn bigsection(
            #[positional_only] self_val: PyValueRef<'_>,
            title: PyStr<'_>,
            #[varargs] args: &[Value],
        ) -> Result<Value> {
            let title_s: &str = &title;
            let mut call_args = Vec::with_capacity(1 + args.len());
            call_args.push(Value::string(format!(
                "<strong class=\"bigsection\">{title_s}</strong>"
            )));
            call_args.extend_from_slice(args);
            call_method(_interp, self_val.0, "section", &call_args)
        }

        fn preformat(#[positional_only] _self: PyValueRef<'_>, text: PyStr<'_>) -> Result<Value> {
            Ok(Value::string(preformat_rs(&text)))
        }

        fn multicolumn(
            #[positional_only] _self: PyValueRef<'_>,
            list: PyValue,
            format: PyValue,
        ) -> Result<Value> {
            let items = _interp.collect_iterable(&list.0)?;
            Ok(Value::string(htmldoc_multicolumn_impl(
                _interp, &items, &format.0,
            )?))
        }

        fn grey(#[positional_only] _self: PyValueRef<'_>, text: PyStr<'_>) -> Result<Value> {
            let text_s: &str = &text;
            Ok(Value::string(format!("<span class=\"grey\">{text_s}</span>")))
        }

        fn namelink(
            #[positional_only] _self: PyValueRef<'_>,
            name: PyStr<'_>,
            #[varargs] dicts: &[Value],
        ) -> Result<Value> {
            Ok(Value::string(htmldoc_namelink_impl(_interp, &name, dicts)?))
        }

        fn classlink(
            #[positional_only] _self: PyValueRef<'_>,
            object: PyValue,
            modname: PyValue,
        ) -> Result<Value> {
            Ok(Value::string(htmldoc_classlink_impl(
                _interp, &object.0, &modname.0,
            )?))
        }

        fn parentlink(
            #[positional_only] _self: PyValueRef<'_>,
            object: PyValue,
            modname: PyValue,
        ) -> Result<Value> {
            htmldoc_parentlink_impl(_interp, &object.0, &modname.0)
        }

        fn modulelink(#[positional_only] _self: PyValueRef<'_>, object: PyValue) -> Result<Value> {
            let name = attr_str(_interp, &object.0, "__name__")?;
            Ok(Value::string(format!("<a href=\"{name}.html\">{name}</a>")))
        }

        fn modpkglink(
            #[positional_only] self_val: PyValueRef<'_>,
            modpkginfo: PyValue,
        ) -> Result<Value> {
            let parts = _interp.collect_iterable(&modpkginfo.0)?;
            if parts.len() != 4 {
                return Err(PyError::named(
                    "ValueError",
                    format!("expected 4 values to unpack, got {}", parts.len()),
                ));
            }
            let name = py_str(_interp, &parts[0])?;
            let path = py_str(_interp, &parts[1])?;
            let ispkg = _interp.truthy_value(&parts[2])?;
            let shadowed = _interp.truthy_value(&parts[3])?;
            if shadowed {
                return call_method(_interp, self_val.0, "grey", &[Value::string(&name)]);
            }
            let url = if path.is_empty() {
                format!("{name}.html")
            } else {
                format!("{path}.{name}.html")
            };
            let text = if ispkg {
                format!("<strong>{name}</strong>&nbsp;(package)")
            } else {
                name
            };
            Ok(Value::string(format!("<a href=\"{url}\">{text}</a>")))
        }

        fn filelink(
            #[positional_only] _self: PyValueRef<'_>,
            url: PyStr<'_>,
            path: PyStr<'_>,
        ) -> Result<Value> {
            let url_s: &str = &url;
            let path_s: &str = &path;
            Ok(Value::string(format!("<a href=\"file:{url_s}\">{path_s}</a>")))
        }

        fn markup(
            #[positional_only] self_val: PyValueRef<'_>,
            text: PyStr<'_>,
            #[default(OptionalArg::Omitted)] escape: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] funcs: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] classes: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] methods: OptValueRef<'_>,
        ) -> Result<Value> {
            let f = funcs
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::dict(PyDict::default()));
            let c = classes
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::dict(PyDict::default()));
            let m = methods
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::dict(PyDict::default()));
            Ok(Value::string(htmldoc_markup_impl(
                _interp,
                self_val.0,
                &text,
                escape.as_value().cloned(),
                f,
                c,
                m,
            )?))
        }

        fn formattree(
            #[positional_only] self_val: PyValueRef<'_>,
            tree: PyValue,
            modname: PyValue,
            #[default(OptionalArg::Omitted)] parent: OptValueRef<'_>,
        ) -> Result<Value> {
            let p = parent.as_value().cloned().unwrap_or_else(Value::none);
            Ok(Value::string(htmldoc_formattree_impl(
                _interp, self_val.0, &tree.0, &modname.0, &p,
            )?))
        }

        fn docmodule(
            #[positional_only] self_val: PyValueRef<'_>,
            object: PyValue,
            #[default(OptionalArg::Omitted)] _name: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _mod: OptValueRef<'_>,
            #[varargs] _ignored: &[Value],
        ) -> Result<Value> {
            Ok(Value::string(htmldoc_docmodule_impl(
                _interp, self_val.0, &object.0,
            )?))
        }

        fn docclass(
            #[positional_only] self_val: PyValueRef<'_>,
            object: PyValue,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] mod_val: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] funcs: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] classes: OptValueRef<'_>,
            #[varargs] _ignored: &[Value],
        ) -> Result<Value> {
            let mv = mod_val.as_value().cloned().unwrap_or_else(Value::none);
            let f = funcs
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::dict(PyDict::default()));
            let c = classes
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::dict(PyDict::default()));
            Ok(Value::string(htmldoc_docclass_impl(
                _interp,
                self_val.0,
                &object.0,
                name.as_value(),
                mv,
                f,
                c,
            )?))
        }

        fn formatvalue(
            #[positional_only] self_val: PyValueRef<'_>,
            object: PyValue,
        ) -> Result<Value> {
            let r = call_method_str(_interp, self_val.0, "repr", &[object.0])?;
            call_method(_interp, self_val.0, "grey", &[Value::string(format!("={r}"))])
        }

        #[allow(clippy::too_many_arguments)]
        fn docroutine(
            #[positional_only] self_val: PyValueRef<'_>,
            object: PyValue,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] mod_val: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] funcs: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] classes: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] methods: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] cl: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] homecls: OptValueRef<'_>,
        ) -> Result<Value> {
            let mv = mod_val.as_value().cloned().unwrap_or_else(Value::none);
            let f = funcs
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::dict(PyDict::default()));
            let c = classes
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::dict(PyDict::default()));
            let m = methods
                .as_value()
                .cloned()
                .unwrap_or_else(|| Value::dict(PyDict::default()));
            Ok(Value::string(htmldoc_docroutine_impl(
                _interp,
                self_val.0,
                &object.0,
                name.as_value(),
                &mv,
                f,
                c,
                m,
                cl.as_value(),
                homecls.as_value(),
            )?))
        }

        fn docdata(
            #[positional_only] self_val: PyValueRef<'_>,
            object: PyValue,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _mod: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _cl: OptValueRef<'_>,
            #[varargs] _ignored: &[Value],
        ) -> Result<Value> {
            Ok(Value::string(htmldoc_docdata_impl(
                _interp,
                self_val.0,
                &object.0,
                name.as_value(),
            )?))
        }

        fn docother(
            #[positional_only] self_val: PyValueRef<'_>,
            object: PyValue,
            #[default(OptionalArg::Omitted)] name: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] _mod: OptValueRef<'_>,
            #[varargs] _ignored: &[Value],
        ) -> Result<Value> {
            Ok(Value::string(htmldoc_docother_impl(
                _interp,
                self_val.0,
                &object.0,
                name.as_value(),
            )?))
        }

        fn index(
            #[positional_only] self_val: PyValueRef<'_>,
            dir: PyValue,
            #[default(OptionalArg::Omitted)] shadowed: OptValueRef<'_>,
        ) -> Result<Value> {
            Ok(Value::string(htmldoc_index_impl(
                _interp,
                self_val.0,
                &dir.0,
                shadowed.as_value().cloned(),
            )?))
        }
    }

    // ── Helper ───────────────────────────────────────────────────────────────

    class Helper {
        constants {
            "keywords" => build_helper_keywords(),
            "symbols" => build_helper_symbols(),
            "topics" => build_helper_topics(),
        }

        fn __init__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] input: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] output: OptValueRef<'_>,
        ) -> Result<Value> {
            let inp = input.as_value().cloned().unwrap_or_else(Value::none);
            let out = output.as_value().cloned().unwrap_or_else(Value::none);
            _interp.assign_attr(self_val.0.clone(), "_input", inp)?;
            _interp.assign_attr(self_val.0.clone(), "_output", out)?;
            Ok(Value::none())
        }

        #[getter]
        fn input(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let inp = py_getattr_opt(_interp, self_val.0, "_input").unwrap_or_else(Value::none);
            if _interp.truthy_value(&inp)? {
                Ok(inp)
            } else {
                let sys_mod = _interp.load_module("sys")?;
                _interp.get_attr(&sys_mod, "stdin")
            }
        }

        #[getter]
        fn output(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let out = py_getattr_opt(_interp, self_val.0, "_output").unwrap_or_else(Value::none);
            if _interp.truthy_value(&out)? {
                Ok(out)
            } else {
                let sys_mod = _interp.load_module("sys")?;
                _interp.get_attr(&sys_mod, "stdout")
            }
        }

        fn __repr__(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let cls = value_class(self_val.0);
            let mod_s = attr_str(_interp, &cls, "__module__")?;
            let qual_s = attr_str(_interp, &cls, "__qualname__")?;
            Ok(Value::string(format!("<{mod_s}.{qual_s} instance>")))
        }

        fn __call__(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] request: OptValueRef<'_>,
        ) -> Result<Value> {
            let sentinel = py_getattr_opt(_interp, self_val.0, "_GoInteractive");
            let go_interactive = match request.as_value() {
                None => true,
                Some(r) => sentinel.as_ref().is_some_and(|s| values_are_identical(r, s)),
            };
            if !go_interactive {
                let req = request.as_value().unwrap().clone();
                match call_method(_interp, self_val.0, "help", &[req]) {
                    Ok(_) => {}
                    Err(err) if is_exc_err(_interp, &err, "ImportError") => {
                        let exc = _interp.materialize_pyerror(err)?;
                        let msg = format!("{}\n", py_str(_interp, &exc)?);
                        helper_write_output(_interp, self_val.0, &msg)?;
                    }
                    Err(err) => return Err(err),
                }
            } else {
                call_method(_interp, self_val.0, "intro", &[])?;
                call_method(_interp, self_val.0, "interact", &[])?;
                helper_write_output(
                    _interp,
                    self_val.0,
                    "\nYou are now leaving help and returning to the Python interpreter.\n\
If you want to ask for help on a particular object directly from the\n\
interpreter, you can type \"help(object)\".  Executing \"help('string')\"\n\
has the same effect as typing a particular string at the help> prompt.\n",
                )?;
            }
            Ok(Value::none())
        }

        fn interact(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            helper_write_output(_interp, self_val.0, "\n")?;
            loop {
                let line_res = call_method(_interp, self_val.0, "getline", &[Value::string("help> ")]);
                let raw = match line_res {
                    Ok(v) => py_str(_interp, &v)?,
                    Err(ref e)
                        if is_exc_err(_interp, e, "KeyboardInterrupt")
                            || is_exc_err(_interp, e, "EOFError") =>
                    {
                        break;
                    }
                    Err(e) => return Err(e),
                };
                if raw.is_empty() {
                    break;
                }
                let mut req = raw.trim().to_string();
                if req.len() > 2 {
                    let first = req.chars().next().unwrap();
                    let last = req.chars().last().unwrap();
                    if (first == '\'' || first == '"')
                        && first == last
                        && !req[1..req.len() - 1].contains(first)
                    {
                        req = req[1..req.len() - 1].to_string();
                    }
                }
                let lower = req.to_lowercase();
                if lower == "q" || lower == "quit" {
                    break;
                }
                if req == "help" {
                    call_method(_interp, self_val.0, "intro", &[])?;
                } else {
                    call_method(_interp, self_val.0, "help", &[Value::string(req)])?;
                }
            }
            Ok(Value::none())
        }

        fn getline(
            #[positional_only] self_val: PyValueRef<'_>,
            prompt: PyStr<'_>,
        ) -> Result<Value> {
            let inp = _interp.get_attr(self_val.0, "input")?;
            let sys_mod = _interp.load_module("sys")?;
            let sys_stdin = _interp.get_attr(&sys_mod, "stdin")?;
            if values_are_identical(&inp, &sys_stdin) {
                call_pos(
                    _interp,
                    Value::builtin_function("input"),
                    &[Value::string(&*prompt)],
                )
            } else {
                let out = _interp.get_attr(self_val.0, "output")?;
                call_method(_interp, &out, "write", &[Value::string(&*prompt)])?;
                let _ = call_method(_interp, &out, "flush", &[]);
                call_method(_interp, &inp, "readline", &[])
            }
        }

        fn help(
            #[positional_only] self_val: PyValueRef<'_>,
            request: PyValue,
            #[default(OptionalArg::Omitted)] is_cli: OptValueRef<'_>,
        ) -> Result<Value> {
            let cli_flag = match is_cli.as_value() {
                Some(v) => _interp.truthy_value(v)?,
                None => false,
            };
            helper_help_impl(_interp, self_val.0, &request.0, cli_flag)?;
            Ok(Value::none())
        }

        fn intro(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            let ver = _interp
                .load_module("sys")
                .and_then(|s| _interp.get_attr(&s, "version_info"))
                .and_then(|vi| _interp.collect_iterable(&vi))
                .ok()
                .and_then(|parts| {
                    let maj = parts.first()?.as_int()?;
                    let min = parts.get(1)?.as_int()?;
                    Some(format!("{maj}.{min}"))
                })
                .unwrap_or_else(|| "3.12".to_string());
            let msg = format!(
                "Welcome to Python {ver}'s help utility! If this is your first time using\n\
Python, you should definitely check out the tutorial at\n\
https://docs.python.org/{ver}/tutorial/.\n\n\
Enter the name of any module, keyword, or topic to get help on writing\n\
Python programs and using Python modules.  To get a list of available\n\
modules, keywords, symbols, or topics, enter \"modules\", \"keywords\",\n\
\"symbols\", or \"topics\".\n\n\
Each module also comes with a one-line summary of what it does; to list\n\
the modules whose name or summary contain a given string such as \"spam\",\n\
enter \"modules spam\".\n\n\
To quit this help utility and return to the interpreter,\n\
enter \"q\" or \"quit\".\n"
            );
            helper_write_output(_interp, self_val.0, &msg)?;
            Ok(Value::none())
        }

        fn list(
            #[positional_only] self_val: PyValueRef<'_>,
            items: PyValue,
            #[default(OptionalArg::Omitted)] columns: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] width: OptValueRef<'_>,
        ) -> Result<Value> {
            let cols = columns
                .as_value()
                .and_then(|v| v.as_int())
                .unwrap_or(4)
                .max(1) as usize;
            let w = width
                .as_value()
                .and_then(|v| v.as_int())
                .unwrap_or(80)
                .max(1) as usize;
            helper_list_impl(_interp, self_val.0, &items.0, cols, w)?;
            Ok(Value::none())
        }

        fn listkeywords(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            helper_write_output(
                _interp,
                self_val.0,
                "\nHere is a list of the Python keywords.  Enter any keyword to get more help.\n\n",
            )?;
            let kw = _interp.get_attr(self_val.0, "keywords")?;
            let keys = call_method(_interp, &kw, "keys", &[])?;
            call_method(_interp, self_val.0, "list", &[keys])?;
            Ok(Value::none())
        }

        fn listsymbols(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            helper_write_output(
                _interp,
                self_val.0,
                "\nHere is a list of the punctuation symbols which Python assigns special meaning\n\
to. Enter any symbol to get more help.\n\n",
            )?;
            let syms = _interp.get_attr(self_val.0, "symbols")?;
            let keys = call_method(_interp, &syms, "keys", &[])?;
            call_method(_interp, self_val.0, "list", &[keys])?;
            Ok(Value::none())
        }

        fn listtopics(#[positional_only] self_val: PyValueRef<'_>) -> Result<Value> {
            helper_write_output(
                _interp,
                self_val.0,
                "\nHere is a list of available topics.  Enter any topic name to get more help.\n\n",
            )?;
            let topics = _interp.get_attr(self_val.0, "topics")?;
            let keys = call_method(_interp, &topics, "keys", &[])?;
            call_method(_interp, self_val.0, "list", &[keys])?;
            Ok(Value::none())
        }

        fn showtopic(
            #[positional_only] self_val: PyValueRef<'_>,
            topic: PyStr<'_>,
            #[default(OptionalArg::Omitted)] more_xrefs: OptValueRef<'_>,
        ) -> Result<Value> {
            let xrefs = match more_xrefs.as_value() {
                Some(v) => py_str(_interp, v)?,
                None => String::new(),
            };
            helper_showtopic_impl(_interp, self_val.0, &topic, &xrefs)?;
            Ok(Value::none())
        }

        fn _gettopic(
            #[positional_only] self_val: PyValueRef<'_>,
            topic: PyStr<'_>,
            #[default(OptionalArg::Omitted)] more_xrefs: OptValueRef<'_>,
        ) -> Result<Value> {
            let xrefs = match more_xrefs.as_value() {
                Some(v) => py_str(_interp, v)?,
                None => String::new(),
            };
            let (doc, xr) = helper_gettopic_impl(_interp, self_val.0, &topic, &xrefs)?;
            Ok(Value::tuple(vec![Value::string(doc), Value::string(xr)]))
        }

        fn showsymbol(#[positional_only] self_val: PyValueRef<'_>, symbol: PyStr<'_>) -> Result<Value> {
            let syms = _interp.get_attr(self_val.0, "symbols")?;
            let target_v = call_method(_interp, &syms, "__getitem__", &[Value::string(&*symbol)])?;
            let target = py_str(_interp, &target_v)?;
            let (topic, xrefs) = match target.split_once(' ') {
                Some((t, x)) => (t, x),
                None => (target.as_str(), ""),
            };
            call_method(
                _interp,
                self_val.0,
                "showtopic",
                &[Value::string(topic), Value::string(xrefs)],
            )?;
            Ok(Value::none())
        }

        fn listmodules(
            #[positional_only] self_val: PyValueRef<'_>,
            #[default(OptionalArg::Omitted)] key: OptValueRef<'_>,
        ) -> Result<Value> {
            let key_s = match key.as_value() {
                Some(v) if _interp.truthy_value(v)? => py_str(_interp, v)?,
                _ => String::new(),
            };
            if !key_s.is_empty() {
                helper_write_output(
                    _interp,
                    self_val.0,
                    &format!(
                        "\nHere is a list of modules whose name or summary contains '{key_s}'.\n\
If there are any, enter a module name to get more help.\n\n"
                    ),
                )?;
                apropos_impl(_interp, &key_s)?;
            } else {
                helper_write_output(
                    _interp,
                    self_val.0,
                    "\nPlease wait a moment while I gather a list of all available modules...\n\n",
                )?;
                let mut names: Vec<String> = Vec::new();
                let sys_mod = _interp.load_module("sys")?;
                let builtin_names = _interp.get_attr(&sys_mod, "builtin_module_names")?;
                for m in _interp.collect_iterable(&builtin_names)? {
                    if let Some(s) = m.as_str()
                        && s != "__main__"
                        && !s.contains('.')
                        && !names.iter().any(|x| x == s)
                    {
                        names.push(s.to_string());
                    }
                }
                if let Ok(pkgutil) = _interp.load_module("pkgutil")
                    && let Ok(walk) = _interp.get_attr(&pkgutil, "walk_packages")
                    && let Ok(it) = call_pos(_interp, walk, &[])
                    && let Ok(entries) = _interp.collect_iterable(&it)
                {
                    for entry in entries {
                        if let Ok(parts) = _interp.collect_iterable(&entry)
                            && let Some(mname) = parts.get(1).and_then(|v| v.as_str())
                        {
                            let display = match mname.strip_suffix(".__init__") {
                                Some(prefix) => format!("{prefix} (package)"),
                                None => mname.to_string(),
                            };
                            if !display.contains('.') && !names.iter().any(|x| x == &display) {
                                names.push(display);
                            }
                        }
                    }
                }
                let list_val = Value::list(names.into_iter().map(Value::string).collect());
                call_method(_interp, self_val.0, "list", &[list_val])?;
                helper_write_output(
                    _interp,
                    self_val.0,
                    "\nEnter any module name to get more help.  Or, type \"modules spam\" to search\n\
for modules whose name or summary contain the string \"spam\".\n",
                )?;
            }
            Ok(Value::none())
        }
    }

    // ── ModuleScanner ────────────────────────────────────────────────────────

    class ModuleScanner {
        fn run(
            #[positional_only] self_val: PyValueRef<'_>,
            callback: PyValue,
            #[default(OptionalArg::Omitted)] key: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] completer: OptValueRef<'_>,
            #[default(OptionalArg::Omitted)] onerror: OptValueRef<'_>,
        ) -> Result<Value> {
            modulescanner_run_impl(
                _interp,
                self_val.0,
                &callback.0,
                key.as_value(),
                completer.as_value(),
                onerror.as_value(),
            )?;
            Ok(Value::none())
        }
    }
}
