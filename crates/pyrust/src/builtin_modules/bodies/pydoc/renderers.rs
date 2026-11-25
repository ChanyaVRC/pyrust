// Renderers for `pydoc`: `Doc`, `TextDoc`, `_PlainTextDoc`, `HTMLDoc`,
// `TextRepr`, `HTMLRepr`, `Helper`, and `ModuleScanner` implementations.

fn doc_getdocloc_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    obj: &Value,
    basedir_opt: Option<Value>,
) -> Result<Value> {
    let basedir = match basedir_opt {
        Some(v) => py_str(interp, &v)?,
        None => interp
            .load_module("sysconfig")
            .and_then(|sc| interp.get_attr(&sc, "get_path"))
            .and_then(|gp| call_pos(interp, gp, &[Value::string("stdlib")]))
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
    };

    let file = match inspect_attr(interp, "getabsfile")
        .and_then(|f| call_pos(interp, f, std::slice::from_ref(obj)))
    {
        Ok(v) => py_str(interp, &v)?,
        Err(ref e) if is_exc_err(interp, e, "TypeError") => "(built-in)".to_string(),
        Err(e) => return Err(e),
    };

    let self_docloc = py_getattr_opt(interp, self_val, "PYTHONDOCS")
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_else(|| "https://docs.python.org/3.12/library".to_string());
    let docloc = interp
        .load_module("os")
        .and_then(|os_mod| interp.get_attr(&os_mod, "environ"))
        .and_then(|env| {
            call_method(
                interp,
                &env,
                "get",
                &[Value::string("PYTHONDOCS"), Value::string(&self_docloc)],
            )
        })
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or(self_docloc);

    let os_path = interp.load_module("os.path")?;
    let normcase = interp.get_attr(&os_path, "normcase")?;
    let join_fn = interp.get_attr(&os_path, "join")?;
    let norm_basedir = call_pos_str(interp, normcase, &[Value::string(&basedir)])?;
    let site_pkg = call_pos_str(
        interp,
        join_fn.clone(),
        &[Value::string(&norm_basedir), Value::string("site-packages")],
    )?;

    if inspect_pred(interp, "ismodule", obj)? {
        let mod_name = attr_str(interp, obj, "__name__")?;
        let in_stdlib = STDLIB_BUILTIN_DOC_MODULES.contains(&mod_name.as_str())
            || (!norm_basedir.is_empty()
                && file.starts_with(&norm_basedir)
                && !file.starts_with(&site_pkg));
        if in_stdlib && mod_name != "xml.etree" && mod_name != "test.test_pydoc.pydoc_mod" {
            let lower = mod_name.to_lowercase();
            if docloc.starts_with("http://") || docloc.starts_with("https://") {
                return Ok(Value::string(format!(
                    "{}/{lower}.html",
                    docloc.trim_end_matches('/')
                )));
            }
            return call_pos(
                interp,
                join_fn,
                &[
                    Value::string(&docloc),
                    Value::string(format!("{lower}.html")),
                ],
            );
        }
    }
    Ok(Value::none())
}

fn doc_document_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    obj: &Value,
    name: Value,
    extra: &[Value],
) -> Result<Value> {
    let mut args = Vec::with_capacity(2 + extra.len());
    args.push(obj.clone());
    args.push(name);
    args.extend_from_slice(extra);

    let try_dispatch =
        |interp: &mut Interpreter, method: &str, args: &[Value]| -> Result<Option<Value>> {
            match call_method(interp, self_val, method, args) {
                Ok(v) => Ok(Some(v)),
                Err(ref e) if is_exc_err(interp, e, "AttributeError") => Ok(None),
                Err(e) => Err(e),
            }
        };

    if inspect_pred(interp, "ismodule", obj)?
        && let Some(v) = try_dispatch(interp, "docmodule", &args)?
    {
        return Ok(v);
    }
    if inspect_pred(interp, "isclass", obj)?
        && let Some(v) = try_dispatch(interp, "docclass", &args)?
    {
        return Ok(v);
    }
    if inspect_pred(interp, "isroutine", obj)?
        && let Some(v) = try_dispatch(interp, "docroutine", &args)?
    {
        return Ok(v);
    }
    if inspect_pred(interp, "isdatadescriptor", obj)? {
        return call_method(interp, self_val, "docdata", &args);
    }
    call_method(interp, self_val, "docother", &args)
}

fn doc_fail_impl(interp: &mut Interpreter, obj: &Value, name: Option<&Value>) -> Result<Value> {
    let name_part = match name {
        Some(n) if interp.truthy_value(n)? => format!(" {}", py_repr(interp, n)?),
        _ => String::new(),
    };
    let type_name = value_type_name_str(obj);
    Err(PyError::named(
        "TypeError",
        format!("don't know how to document object{name_part} of type {type_name}"),
    ))
}

// ── TextDoc implementation ───────────────────────────────────────────────────

fn textdoc_formattree_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    tree: &Value,
    modname: &Value,
    parent: &Value,
    prefix: &str,
) -> Result<String> {
    let mut result = String::new();
    let mut last_c = Value::none();
    for entry in interp.collect_iterable(tree)? {
        if is_tuple_val(interp, &entry) {
            let parts = interp.collect_iterable(&entry)?;
            if parts.len() == 2 {
                let c = parts[0].clone();
                let bases = parts[1].clone();
                result.push_str(prefix);
                result.push_str(&classname_impl(interp, &c, modname)?);
                let bases_items = interp.collect_iterable(&bases)?;
                let same_as_parent =
                    bases_items.len() == 1 && values_are_identical(&bases_items[0], parent);
                if !bases_items.is_empty() && !same_as_parent {
                    let mut parents = Vec::with_capacity(bases_items.len());
                    for base in &bases_items {
                        parents.push(classname_impl(interp, base, modname)?);
                    }
                    result.push('(');
                    result.push_str(&parents.join(", "));
                    result.push(')');
                }
                result.push('\n');
                last_c = c;
            }
        } else if is_list_val(interp, &entry) {
            let sub = call_method(
                interp,
                self_val,
                "formattree",
                &[
                    entry,
                    modname.clone(),
                    last_c.clone(),
                    Value::string(format!("{prefix}    ")),
                ],
            )?;
            result.push_str(&py_str(interp, &sub)?);
        }
    }
    Ok(result)
}

fn textdoc_section_call(
    interp: &mut Interpreter,
    self_val: &Value,
    title: &str,
    contents: &str,
) -> Result<String> {
    let v = call_method(
        interp,
        self_val,
        "section",
        &[Value::string(title), Value::string(contents)],
    )?;
    py_str(interp, &v)
}

fn textdoc_docmodule_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    obj: &Value,
) -> Result<String> {
    let name_val = interp.get_attr(obj, "__name__")?;
    let name = py_str(interp, &name_val)?;
    let (synop, desc) = splitdoc_rs(&getdoc_impl(interp, obj)?);
    let name_line = if synop.is_empty() {
        name.clone()
    } else {
        format!("{name} - {synop}")
    };
    let mut result = textdoc_section_call(interp, self_val, "NAME", &name_line)?;
    let all = py_getattr_opt(interp, obj, "__all__");
    let docloc = call_method(interp, self_val, "getdocloc", std::slice::from_ref(obj))?;
    if !docloc.is_none() {
        let docloc_s = py_str(interp, &docloc)?;
        let ref_text = format!(
            "{docloc_s}\n\n\
The following documentation is automatically generated from the Python\n\
source files.  It may be incomplete, incorrect or include features that\n\
are considered implementation detail and may vary between Python\n\
implementations.  When in doubt, consult the module reference at the\n\
location listed above.\n"
        );
        result.push_str(&textdoc_section_call(
            interp,
            self_val,
            "MODULE REFERENCE",
            &ref_text,
        )?);
    }
    if !desc.is_empty() {
        result.push_str(&textdoc_section_call(
            interp,
            self_val,
            "DESCRIPTION",
            &desc,
        )?);
    }

    let getmembers = inspect_attr(interp, "getmembers")?;
    let getmodule = inspect_attr(interp, "getmodule")?;

    let mut classes: Vec<(String, Value)> = Vec::new();
    let isclass_fn = inspect_attr(interp, "isclass")?;
    let class_members = call_pos(interp, getmembers.clone(), &[obj.clone(), isclass_fn])?;
    for pair in interp.collect_iterable(&class_members)? {
        let items = interp.collect_iterable(&pair)?;
        if items.len() != 2 {
            continue;
        }
        let key = py_str(interp, &items[0])?;
        let val = items[1].clone();
        let include_mod = if all.is_some() {
            true
        } else {
            let m = call_pos(interp, getmodule.clone(), std::slice::from_ref(&val))?;
            m.is_none() || values_are_identical(&m, obj)
        };
        if include_mod {
            let vis = visiblename_impl(interp, &key, all.as_ref(), Some(obj))?;
            if interp.truthy_value(&vis)? {
                classes.push((key, val));
            }
        }
    }

    let mut funcs: Vec<(String, Value)> = Vec::new();
    let isroutine_fn = inspect_attr(interp, "isroutine")?;
    let func_members = call_pos(interp, getmembers.clone(), &[obj.clone(), isroutine_fn])?;
    for pair in interp.collect_iterable(&func_members)? {
        let items = interp.collect_iterable(&pair)?;
        if items.len() != 2 {
            continue;
        }
        let key = py_str(interp, &items[0])?;
        let val = items[1].clone();
        let include_mod = if all.is_some() || inspect_pred(interp, "isbuiltin", &val)? {
            true
        } else {
            let m = call_pos(interp, getmodule.clone(), std::slice::from_ref(&val))?;
            values_are_identical(&m, obj)
        };
        if include_mod {
            let vis = visiblename_impl(interp, &key, all.as_ref(), Some(obj))?;
            if interp.truthy_value(&vis)? {
                funcs.push((key, val));
            }
        }
    }

    let mut data: Vec<(String, Value)> = Vec::new();
    let isdata_fn = pydoc_attr(interp, "isdata")?;
    let data_members = call_pos(interp, getmembers.clone(), &[obj.clone(), isdata_fn])?;
    for pair in interp.collect_iterable(&data_members)? {
        let items = interp.collect_iterable(&pair)?;
        if items.len() != 2 {
            continue;
        }
        let key = py_str(interp, &items[0])?;
        let val = items[1].clone();
        let vis = visiblename_impl(interp, &key, all.as_ref(), Some(obj))?;
        if interp.truthy_value(&vis)? {
            data.push((key, val));
        }
    }

    let mut modpkgs: Vec<String> = Vec::new();
    let mut modpkgs_names: Vec<String> = Vec::new();
    if let Some(pkg_path) = py_getattr_opt(interp, obj, "__path__") {
        let pkgutil = interp.load_module("pkgutil")?;
        let iter_modules = interp.get_attr(&pkgutil, "iter_modules")?;
        let iter = call_pos(interp, iter_modules, &[pkg_path])?;
        for entry in interp.collect_iterable(&iter)? {
            let parts = interp.collect_iterable(&entry)?;
            if parts.len() >= 3 {
                let mname = py_str(interp, &parts[1])?;
                let ispkg = interp.truthy_value(&parts[2])?;
                modpkgs_names.push(mname.clone());
                if ispkg {
                    modpkgs.push(format!("{mname} (package)"));
                } else {
                    modpkgs.push(mname);
                }
            }
        }
        modpkgs.sort();
        result.push_str(&textdoc_section_call(
            interp,
            self_val,
            "PACKAGE CONTENTS",
            &modpkgs.join("\n"),
        )?);
    }

    let mut submodules: Vec<String> = Vec::new();
    let ismodule_fn = inspect_attr(interp, "ismodule")?;
    let mod_members = call_pos(interp, getmembers, &[obj.clone(), ismodule_fn])?;
    let prefix = format!("{name}.");
    for pair in interp.collect_iterable(&mod_members)? {
        let items = interp.collect_iterable(&pair)?;
        if items.len() != 2 {
            continue;
        }
        let key = py_str(interp, &items[0])?;
        let val = items[1].clone();
        if let Some(sub_name) =
            py_getattr_opt(interp, &val, "__name__").and_then(|v| v.as_str().map(str::to_owned))
            && sub_name.starts_with(&prefix)
            && !modpkgs_names.contains(&key)
        {
            submodules.push(key);
        }
    }
    if !submodules.is_empty() {
        submodules.sort();
        result.push_str(&textdoc_section_call(
            interp,
            self_val,
            "SUBMODULES",
            &submodules.join("\n"),
        )?);
    }

    if !classes.is_empty() {
        let classlist = Value::list(classes.iter().map(|(_, v)| v.clone()).collect());
        let getclasstree = inspect_attr(interp, "getclasstree")?;
        let tree = call_pos(interp, getclasstree, &[classlist, Value::int(1)])?;
        let tree_str = call_method_str(interp, self_val, "formattree", &[tree, name_val.clone()])?;
        let mut contents = vec![tree_str];
        for (key, val) in &classes {
            let doc = call_method(
                interp,
                self_val,
                "document",
                &[val.clone(), Value::string(key), name_val.clone()],
            )?;
            contents.push(py_str(interp, &doc)?);
        }
        result.push_str(&textdoc_section_call(
            interp,
            self_val,
            "CLASSES",
            &contents.join("\n"),
        )?);
    }

    if !funcs.is_empty() {
        let mut contents = Vec::with_capacity(funcs.len());
        for (key, val) in &funcs {
            let doc = call_method(
                interp,
                self_val,
                "document",
                &[val.clone(), Value::string(key), name_val.clone()],
            )?;
            contents.push(py_str(interp, &doc)?);
        }
        result.push_str(&textdoc_section_call(
            interp,
            self_val,
            "FUNCTIONS",
            &contents.join("\n"),
        )?);
    }

    if !data.is_empty() {
        let mut contents = Vec::with_capacity(data.len());
        let docother_fn = interp.get_attr(self_val, "docother")?;
        for (key, val) in &data {
            let doc = call_kw(
                interp,
                docother_fn.clone(),
                &[val.clone(), Value::string(key), name_val.clone()],
                &[("maxlen", Value::int(70))],
            )?;
            contents.push(py_str(interp, &doc)?);
        }
        result.push_str(&textdoc_section_call(
            interp,
            self_val,
            "DATA",
            &contents.join("\n"),
        )?);
    }

    if let Some(ver) = py_getattr_opt(interp, obj, "__version__") {
        let v_str = format_version_str(&py_str(interp, &ver)?);
        result.push_str(&textdoc_section_call(interp, self_val, "VERSION", &v_str)?);
    }
    if let Some(d) = py_getattr_opt(interp, obj, "__date__") {
        let s = py_str(interp, &d)?;
        result.push_str(&textdoc_section_call(interp, self_val, "DATE", &s)?);
    }
    if let Some(a) = py_getattr_opt(interp, obj, "__author__") {
        let s = py_str(interp, &a)?;
        result.push_str(&textdoc_section_call(interp, self_val, "AUTHOR", &s)?);
    }
    if let Some(c) = py_getattr_opt(interp, obj, "__credits__") {
        let s = py_str(interp, &c)?;
        result.push_str(&textdoc_section_call(interp, self_val, "CREDITS", &s)?);
    }

    let file = match inspect_attr(interp, "getabsfile")
        .and_then(|f| call_pos(interp, f, std::slice::from_ref(obj)))
    {
        Ok(v) => py_str(interp, &v)?,
        Err(ref e) if is_exc_err(interp, e, "TypeError") => "(built-in)".to_string(),
        Err(e) => return Err(e),
    };
    result.push_str(&textdoc_section_call(interp, self_val, "FILE", &file)?);
    Ok(result)
}

fn textdoc_docclass_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    obj: &Value,
    name_opt: Option<&Value>,
    mod_val: Value,
) -> Result<String> {
    let realname = attr_str(interp, obj, "__name__")?;
    let name = match name_opt {
        Some(n) if interp.truthy_value(n)? => py_str(interp, n)?,
        _ => realname.clone(),
    };
    let bases = interp.get_attr(obj, "__bases__")?;
    let obj_mod = interp.get_attr(obj, "__module__")?;

    let bold_fn = interp.get_attr(self_val, "bold")?;
    let mut title = if name == realname {
        let b = call_pos_str(interp, bold_fn, &[Value::string(&realname)])?;
        format!("class {b}")
    } else {
        let b = call_pos_str(interp, bold_fn, &[Value::string(&name)])?;
        format!("{b} = class {realname}")
    };
    let bases_items = interp.collect_iterable(&bases)?;
    if !bases_items.is_empty() {
        let mut parents = Vec::with_capacity(bases_items.len());
        for base in &bases_items {
            parents.push(classname_impl(interp, base, &obj_mod)?);
        }
        title.push('(');
        title.push_str(&parents.join(", "));
        title.push(')');
    }

    let mut contents: Vec<String> = Vec::new();
    let sig_fn = inspect_attr(interp, "signature")?;
    if let Ok(sig) = call_pos(interp, sig_fn, std::slice::from_ref(obj)) {
        let argspec = py_str(interp, &sig)?;
        if !argspec.is_empty() && argspec != "()" {
            contents.push(format!("{name}{argspec}\n"));
        }
    }

    let doc = getdoc_impl(interp, obj)?;
    if !doc.is_empty() {
        contents.push(format!("{doc}\n"));
    }

    let getmro = inspect_attr(interp, "getmro")?;
    let mro_val = call_pos(interp, getmro, std::slice::from_ref(obj))?;
    let mut mro: std::collections::VecDeque<Value> = interp.collect_iterable(&mro_val)?.into();
    if mro.len() > 2 {
        contents.push("Method resolution order:".to_string());
        for base in &mro {
            contents.push(format!("    {}", classname_impl(interp, base, &obj_mod)?));
        }
        contents.push(String::new());
    }

    // Built-in subclasses (matching CPython 3.12)
    let type_cls = Value::py_class(type_class_singleton());
    if let Ok(subclasses_fn) = interp.get_attr(&type_cls, "__subclasses__")
        && let Ok(subs_val) = call_pos(interp, subclasses_fn, std::slice::from_ref(obj))
        && let Ok(subs) = interp.collect_iterable(&subs_val)
    {
        let mut sub_names: Vec<String> = Vec::new();
        for cls in subs {
            let cname = py_getattr_opt(interp, &cls, "__name__")
                .and_then(|v| v.as_str().map(str::to_owned));
            let cmod = py_getattr_opt(interp, &cls, "__module__")
                .and_then(|v| v.as_str().map(str::to_owned));
            if let (Some(cn), Some(cm)) = (cname, cmod)
                && !cn.starts_with('_')
                && cm == "builtins"
            {
                sub_names.push(cn);
            }
        }
        sub_names.sort_by_key(|s| s.to_lowercase());
        if !sub_names.is_empty() {
            const MAX_SUBCLASSES: usize = 4;
            contents.push("Built-in subclasses:".to_string());
            for sub in sub_names.iter().take(MAX_SUBCLASSES) {
                contents.push(format!("    {sub}"));
            }
            if sub_names.len() > MAX_SUBCLASSES {
                contents.push(format!(
                    "    ... and {} other subclasses",
                    sub_names.len() - MAX_SUBCLASSES
                ));
            }
            contents.push(String::new());
        }
    }

    let mut need_hr = false;
    let maybe_hr = |need: &mut bool, contents: &mut Vec<String>| {
        if *need {
            contents.push("-".repeat(70));
        }
        *need = true;
    };

    let all_attrs = classify_class_attrs_impl(interp, obj)?;
    let mut attrs: Vec<(String, String, Value, Value)> = Vec::new();
    for entry in all_attrs {
        let vis = visiblename_impl(interp, &entry.0, None, Some(obj))?;
        if interp.truthy_value(&vis)? {
            attrs.push(entry);
        }
    }

    let builtins_object = Value::py_class(object_class_singleton());
    while !attrs.is_empty() {
        let thisclass = mro.pop_front().unwrap_or_else(|| attrs[0].2.clone());
        let (mut cur, inherited): (Vec<_>, Vec<_>) = attrs
            .into_iter()
            .partition(|t| values_are_identical(&t.2, &thisclass));

        if !values_are_identical(obj, &builtins_object)
            && values_are_identical(&thisclass, &builtins_object)
        {
            attrs = inherited;
            continue;
        }
        let tag = if values_are_identical(&thisclass, obj) {
            "defined here".to_string()
        } else {
            format!(
                "inherited from {}",
                classname_impl(interp, &thisclass, &obj_mod)?
            )
        };

        sort_attributes_vec(interp, &mut cur, obj);

        for (heading_prefix, kind_match) in [
            ("Methods", "method"),
            ("Class methods", "class method"),
            ("Static methods", "static method"),
        ] {
            let (ok, rest): (Vec<_>, Vec<_>) = cur.into_iter().partition(|t| t.1 == kind_match);
            cur = rest;
            if !ok.is_empty() {
                maybe_hr(&mut need_hr, &mut contents);
                contents.push(format!("{heading_prefix} {tag}:\n"));
                for (aname, _kind, homecls, value) in ok {
                    let rendered = match interp.get_attr(obj, &aname) {
                        Ok(val) => call_method(
                            interp,
                            self_val,
                            "document",
                            &[
                                val,
                                Value::string(&aname),
                                mod_val.clone(),
                                obj.clone(),
                                homecls,
                            ],
                        )?,
                        Err(_) => call_method(
                            interp,
                            self_val,
                            "docdata",
                            &[value, Value::string(&aname), mod_val.clone()],
                        )?,
                    };
                    contents.push(py_str(interp, &rendered)?);
                }
            }
        }

        for (heading_prefix, kind_match) in [
            ("Readonly properties", "readonly property"),
            ("Data descriptors", "data descriptor"),
        ] {
            let (ok, rest): (Vec<_>, Vec<_>) = cur.into_iter().partition(|t| t.1 == kind_match);
            cur = rest;
            if !ok.is_empty() {
                maybe_hr(&mut need_hr, &mut contents);
                contents.push(format!("{heading_prefix} {tag}:\n"));
                for (aname, _kind, _homecls, value) in ok {
                    let rendered = call_method(
                        interp,
                        self_val,
                        "docdata",
                        &[value, Value::string(&aname), mod_val.clone()],
                    )?;
                    contents.push(py_str(interp, &rendered)?);
                }
            }
        }

        let (ok, _rest): (Vec<_>, Vec<_>) = cur.into_iter().partition(|t| t.1 == "data");
        if !ok.is_empty() {
            maybe_hr(&mut need_hr, &mut contents);
            contents.push(format!("Data and other attributes {tag}:\n"));
            let docother_fn = interp.get_attr(self_val, "docother")?;
            for (aname, _kind, homecls, value) in ok {
                let d = getdoc_impl(interp, &value)?;
                let item_obj = match interp.get_attr(obj, &aname) {
                    Ok(v) => v,
                    Err(_) => interp
                        .get_attr(&homecls, "__dict__")
                        .and_then(|dict| {
                            call_method(interp, &dict, "__getitem__", &[Value::string(&aname)])
                        })
                        .unwrap_or(value),
                };
                let rendered = call_kw(
                    interp,
                    docother_fn.clone(),
                    &[item_obj, Value::string(&aname), mod_val.clone()],
                    &[("maxlen", Value::int(70)), ("doc", Value::string(d))],
                )?;
                contents.push(format!("{}\n", py_str(interp, &rendered)?));
            }
        }

        attrs = inherited;
    }

    let body = contents.join("\n");
    if body.is_empty() {
        return Ok(format!("{title}\n"));
    }
    let indented = call_method(
        interp,
        self_val,
        "indent",
        &[Value::string(body.trim_end()), Value::string(" |  ")],
    )?;
    Ok(format!("{title}\n{}\n", py_str(interp, &indented)?))
}

fn textdoc_docroutine_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    obj: &Value,
    name_opt: Option<&Value>,
    mod_val: &Value,
    cl: Option<&Value>,
    homecls_opt: Option<&Value>,
) -> Result<String> {
    let realname = attr_str(interp, obj, "__name__")?;
    let name = match name_opt {
        Some(n) if interp.truthy_value(n)? => py_str(interp, n)?,
        _ => realname.clone(),
    };
    let cl_opt = cl.filter(|v| !v.is_none());
    let homecls = homecls_opt.filter(|v| !v.is_none()).or(cl_opt);
    let mut note = String::new();
    let mut skipdocs = false;
    let mut imfunc: Option<Value> = None;

    if is_bound_method_impl(interp, obj)? {
        let imself = interp.get_attr(obj, "__self__")?;
        if cl_opt.is_some_and(|c| values_are_identical(&imself, c)) {
            imfunc = py_getattr_opt(interp, obj, "__func__");
        } else if inspect_pred(interp, "isclass", &imself)? {
            note = format!(
                " class method of {}",
                classname_impl(interp, &imself, mod_val)?
            );
        } else {
            let imcls = interp.get_attr(&imself, "__class__")?;
            note = format!(
                " method of {} instance",
                classname_impl(interp, &imcls, mod_val)?
            );
        }
    } else if inspect_pred(interp, "ismethoddescriptor", obj)?
        || inspect_pred(interp, "ismethodwrapper", obj)?
    {
        if let Some(objclass) = py_getattr_opt(interp, obj, "__objclass__") {
            if cl_opt.is_none() {
                note = format!(
                    " unbound {} method",
                    classname_impl(interp, &objclass, mod_val)?
                );
            } else if homecls.is_none_or(|hc| !values_are_identical(&objclass, hc)) {
                note = format!(" from {}", classname_impl(interp, &objclass, mod_val)?);
            }
        }
    } else {
        imfunc = Some(obj.clone());
    }

    if let Some(ref f) = imfunc
        && inspect_pred(interp, "isfunction", f)?
        && let Some(hc) = homecls
    {
        let f_mod =
            py_getattr_opt(interp, f, "__module__").and_then(|v| v.as_str().map(str::to_owned));
        let hc_mod =
            py_getattr_opt(interp, hc, "__module__").and_then(|v| v.as_str().map(str::to_owned));
        let f_qual =
            py_getattr_opt(interp, f, "__qualname__").and_then(|v| v.as_str().map(str::to_owned));
        let hc_qual =
            py_getattr_opt(interp, hc, "__qualname__").and_then(|v| v.as_str().map(str::to_owned));
        if f_mod != hc_mod
            || f_qual.as_deref()
                != hc_qual
                    .as_deref()
                    .map(|q| format!("{q}.{realname}"))
                    .as_deref()
        {
            let pname = parentname_impl(interp, f, mod_val)?;
            if interp.truthy_value(&pname)? {
                note = format!(" from {}", py_str(interp, &pname)?);
            }
        }
    }

    let asyncqualifier = if inspect_pred(interp, "iscoroutinefunction", obj)?
        || inspect_pred(interp, "isasyncgenfunction", obj)?
    {
        "async "
    } else {
        ""
    };

    let bold_fn = interp.get_attr(self_val, "bold")?;
    let mut title = if name == realname {
        call_pos_str(interp, bold_fn.clone(), &[Value::string(&realname)])?
    } else {
        if let Some(c) = cl_opt
            && let Ok(gs) = inspect_attr(interp, "getattr_static")
            && let Ok(static_val) = call_pos(
                interp,
                gs,
                &[c.clone(), Value::string(&realname), Value::list(Vec::new())],
            )
            && values_are_identical(&static_val, obj)
        {
            skipdocs = true;
            if note.starts_with(" from ") {
                note.clear();
            }
        }
        let b = call_pos_str(interp, bold_fn.clone(), &[Value::string(&name)])?;
        format!("{b} = {realname}")
    };

    let mut argspec: Option<String> = None;
    if inspect_pred(interp, "isroutine", obj)?
        && let Ok(sig_fn) = inspect_attr(interp, "signature")
        && let Ok(sig) = call_pos(interp, sig_fn, std::slice::from_ref(obj))
    {
        let mut s = py_str(interp, &sig)?;
        if realname == "<lambda>" {
            let b = call_pos_str(interp, bold_fn, &[Value::string(&name)])?;
            title = format!("{b} lambda ");
            let has_ann = py_getattr_opt(interp, obj, "__annotations__")
                .and_then(|v| interp.truthy_value(&v).ok())
                .unwrap_or(false);
            if !has_ann && s.len() >= 2 && s.starts_with('(') && s.ends_with(')') {
                s = s[1..s.len() - 1].to_string();
            }
        }
        if !s.is_empty() {
            argspec = Some(s);
        }
    }
    let argspec = argspec.unwrap_or_else(|| "(...)".to_string());
    let decl = format!("{asyncqualifier}{title}{argspec}{note}");

    if skipdocs {
        Ok(format!("{decl}\n"))
    } else {
        let doc = getdoc_impl(interp, obj)?;
        if doc.is_empty() {
            Ok(format!("{decl}\n"))
        } else {
            let indented = call_method_str(interp, self_val, "indent", &[Value::string(&doc)])?;
            Ok(format!("{decl}\n{}\n", indented.trim_end()))
        }
    }
}

fn textdoc_docdata_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    obj: &Value,
    name_opt: Option<&Value>,
) -> Result<String> {
    let mut out = String::new();
    if let Some(n) = name_opt
        && interp.truthy_value(n)?
    {
        let bold_v = call_method(interp, self_val, "bold", std::slice::from_ref(n))?;
        out.push_str(&py_str(interp, &bold_v)?);
        out.push('\n');
    }
    let doc = getdoc_impl(interp, obj)?;
    if !doc.is_empty() {
        let ind = call_method(interp, self_val, "indent", &[Value::string(&doc)])?;
        out.push_str(&py_str(interp, &ind)?);
        out.push('\n');
    }
    Ok(out)
}

fn textdoc_docother_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    obj: &Value,
    name_opt: Option<&Value>,
    maxlen_opt: Option<usize>,
    doc_opt: Option<Value>,
) -> Result<String> {
    let mut repr_s = call_method_str(interp, self_val, "repr", std::slice::from_ref(obj))?;
    let name_str = match name_opt {
        Some(n) if interp.truthy_value(n)? => Some(py_str(interp, n)?),
        _ => None,
    };
    if let Some(maxlen) = maxlen_opt {
        let line_plain = match &name_str {
            Some(n) => format!("{n} = {repr_s}"),
            None => repr_s.clone(),
        };
        let line_len = line_plain.chars().count();
        if line_len > maxlen {
            let repr_chars: Vec<char> = repr_s.chars().collect();
            let drop_n = line_len - maxlen;
            let keep = repr_chars.len().saturating_sub(drop_n);
            repr_s = repr_chars[..keep].iter().collect::<String>() + "...";
        }
    }
    let mut line = match &name_str {
        Some(n) => {
            let b = call_method_str(interp, self_val, "bold", &[Value::string(n)])?;
            format!("{b} = {repr_s}")
        }
        None => repr_s,
    };
    let doc_text = match doc_opt {
        Some(d) if interp.truthy_value(&d)? => py_str(interp, &d)?,
        _ => getdoc_impl(interp, obj)?,
    };
    if !doc_text.is_empty() {
        let ind = call_method_str(interp, self_val, "indent", &[Value::string(&doc_text)])?;
        line.push('\n');
        line.push_str(&ind);
        line.push('\n');
    }
    Ok(line)
}

// ── HTMLDoc implementation ───────────────────────────────────────────────────

fn htmldoc_section_impl(
    title: &str,
    cls: &str,
    contents: &str,
    width: usize,
    prelude: &str,
    marginalia: Option<&str>,
    gap: &str,
) -> String {
    let marg = match marginalia {
        Some(m) => m.to_string(),
        None => format!("<span class=\"code\">{}</span>", "&nbsp;".repeat(width)),
    };
    let mut result = format!(
        "<p>\n\
<table class=\"section\">\n\
<tr class=\"decor {cls}-decor heading-text\">\n\
<td class=\"section-title\" colspan=3>&nbsp;<br>{title}</td></tr>\n    "
    );
    if !prelude.is_empty() {
        result.push_str(&format!(
            "\n<tr><td class=\"decor {cls}-decor\" rowspan=2>{marg}</td>\n\
<td class=\"decor {cls}-decor\" colspan=2>{prelude}</td></tr>\n\
<tr><td>{gap}</td>"
        ));
    } else {
        result.push_str(&format!(
            "\n<tr><td class=\"decor {cls}-decor\">{marg}</td><td>{gap}</td>"
        ));
    }
    result.push_str(&format!(
        "\n<td class=\"singlecolumn\">{contents}</td></tr></table>"
    ));
    result
}

fn htmldoc_multicolumn_impl(
    interp: &mut Interpreter,
    items: &[Value],
    format_fn: &Value,
) -> Result<String> {
    let mut result = String::new();
    let rows = items.len().div_ceil(4);
    for col in 0..4 {
        result.push_str("<td class=\"multicolumn\">");
        for i in (rows * col)..(rows * col + rows) {
            if let Some(item) = items.get(i) {
                let formatted = call_pos(interp, format_fn.clone(), std::slice::from_ref(item))?;
                result.push_str(&py_str(interp, &formatted)?);
                result.push_str("<br>\n");
            }
        }
        result.push_str("</td>");
    }
    Ok(format!("<table><tr>{result}</tr></table>"))
}

fn htmldoc_namelink_impl(interp: &mut Interpreter, name: &str, dicts: &[Value]) -> Result<String> {
    let name_v = Value::string(name);
    for d in dicts {
        let contains = interp.eval_in(d.clone(), name_v.clone())?;
        if interp.truthy_value(&contains)? {
            let url = call_method(interp, d, "__getitem__", std::slice::from_ref(&name_v))?;
            let url_s = py_str(interp, &url)?;
            return Ok(format!("<a href=\"{url_s}\">{name}</a>"));
        }
    }
    Ok(name.to_string())
}

fn htmldoc_classlink_impl(
    interp: &mut Interpreter,
    obj: &Value,
    modname: &Value,
) -> Result<String> {
    let name = attr_str(interp, obj, "__name__")?;
    let obj_mod = interp.get_attr(obj, "__module__")?;
    let sys_mod = interp.load_module("sys")?;
    let modules = interp.get_attr(&sys_mod, "modules")?;
    let module = call_method(interp, &modules, "get", &[obj_mod])?;
    let display = classname_impl(interp, obj, modname)?;
    if !module.is_none()
        && let Ok(attr) = interp.get_attr(&module, &name)
        && values_are_identical(&attr, obj)
    {
        let mname = attr_str(interp, &module, "__name__")?;
        return Ok(format!("<a href=\"{mname}.html#{name}\">{display}</a>"));
    }
    Ok(display)
}

fn htmldoc_parentlink_impl(
    interp: &mut Interpreter,
    obj: &Value,
    modname: &Value,
) -> Result<Value> {
    let mut link: Option<String> = None;
    let name = attr_str(interp, obj, "__name__")?;
    let obj_mod = interp.get_attr(obj, "__module__")?;
    let obj_mod_s = py_str(interp, &obj_mod)?;
    let same_mod = modname.as_str().is_some_and(|m| m == obj_mod_s);
    let sys_mod = interp.load_module("sys")?;
    let modules = interp.get_attr(&sys_mod, "modules")?;
    let module = call_method(interp, &modules, "get", &[obj_mod])?;
    if !module.is_none()
        && let Ok(attr) = interp.get_attr(&module, &name)
        && values_are_identical(&attr, obj)
    {
        let mname = attr_str(interp, &module, "__name__")?;
        let qualname = attr_str(interp, obj, "__qualname__")?;
        if let Some((prefix, _)) = qualname.rsplit_once('.') {
            if !same_mod {
                link = Some(format!("{mname}.html#{prefix}"));
            } else {
                link = Some(format!("#{prefix}"));
            }
        } else if !same_mod {
            link = Some(format!("{mname}.html"));
        }
    }
    let pname = parentname_impl(interp, obj, modname)?;
    if let Some(l) = link {
        let p_s = py_str(interp, &pname)?;
        Ok(Value::string(format!("<a href=\"{l}\">{p_s}</a>")))
    } else {
        Ok(pname)
    }
}

fn htmldoc_markup_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    text: &str,
    escape_opt: Option<Value>,
    funcs: Value,
    classes: Value,
    methods: Value,
) -> Result<String> {
    let escape_fn = match escape_opt.filter(|v| !v.is_none()) {
        Some(f) => f,
        None => interp.get_attr(self_val, "escape")?,
    };
    let call_escape = |interp: &mut Interpreter, s: &str| -> Result<String> {
        let v = call_pos(interp, escape_fn.clone(), &[Value::string(s)])?;
        py_str(interp, &v)
    };

    let mut results = String::with_capacity(text.len());
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    let mut here_byte = 0usize;
    let mut idx = 0usize;

    while idx < n {
        let (byte_pos, ch) = chars[idx];
        let at_word_boundary = is_word_char(ch) && (idx == 0 || !is_word_char(chars[idx - 1].1));
        if !at_word_boundary {
            idx += 1;
            continue;
        }
        let slice = &text[byte_pos..];

        // 1. URL: (http|https|ftp)://\S+[\w/]
        if let Some(scheme_len) = if slice.starts_with("http://") {
            Some(7)
        } else if slice.starts_with("https://") {
            Some(8)
        } else if slice.starts_with("ftp://") {
            Some(6)
        } else {
            None
        } {
            let mut end_idx = idx;
            while end_idx < n && !chars[end_idx].1.is_whitespace() {
                end_idx += 1;
            }
            let mut trimmed_end = end_idx;
            while trimmed_end > idx {
                let last_c = chars[trimmed_end - 1].1;
                if is_word_char(last_c) || last_c == '/' {
                    break;
                }
                trimmed_end -= 1;
            }
            let end_byte = if trimmed_end < n {
                chars[trimmed_end].0
            } else {
                text.len()
            };
            if end_byte > byte_pos + scheme_len + 1 {
                results.push_str(&call_escape(interp, &text[here_byte..byte_pos])?);
                let url_raw = &text[byte_pos..end_byte];
                let url = call_escape(interp, url_raw)?.replace('"', "&quot;");
                results.push_str(&format!("<a href=\"{url}\">{url}</a>"));
                here_byte = end_byte;
                idx = trimmed_end;
                continue;
            }
        }

        // 2. RFC[- ]?(\d+)
        if let Some(rest) = slice.strip_prefix("RFC") {
            let mut offset = 3usize;
            if rest.starts_with('-') || rest.starts_with(' ') {
                offset += 1;
            }
            let digits_start = byte_pos + offset;
            let mut d_end = digits_start;
            for c in text[digits_start..].chars() {
                if c.is_ascii_digit() {
                    d_end += c.len_utf8();
                } else {
                    break;
                }
            }
            if d_end > digits_start {
                let num: u64 = text[digits_start..d_end].parse().unwrap_or(0);
                results.push_str(&call_escape(interp, &text[here_byte..byte_pos])?);
                let label = call_escape(interp, &text[byte_pos..d_end])?;
                results.push_str(&format!(
                    "<a href=\"https://www.rfc-editor.org/rfc/rfc{num}.txt\">{label}</a>"
                ));
                here_byte = d_end;
                while idx < n && chars[idx].0 < d_end {
                    idx += 1;
                }
                continue;
            }
        }

        // 3. PEP[- ]?(\d+)
        if let Some(rest) = slice.strip_prefix("PEP") {
            let mut offset = 3usize;
            if rest.starts_with('-') || rest.starts_with(' ') {
                offset += 1;
            }
            let digits_start = byte_pos + offset;
            let mut d_end = digits_start;
            for c in text[digits_start..].chars() {
                if c.is_ascii_digit() {
                    d_end += c.len_utf8();
                } else {
                    break;
                }
            }
            if d_end > digits_start {
                let num: u64 = text[digits_start..d_end].parse().unwrap_or(0);
                results.push_str(&call_escape(interp, &text[here_byte..byte_pos])?);
                let label = call_escape(interp, &text[byte_pos..d_end])?;
                results.push_str(&format!(
                    "<a href=\"https://peps.python.org/pep-{num:04}/\">{label}</a>"
                ));
                here_byte = d_end;
                while idx < n && chars[idx].0 < d_end {
                    idx += 1;
                }
                continue;
            }
        }

        // 4. (self\.)?(\w+)
        let (selfdot, name_start_idx) =
            if slice.starts_with("self.") && idx + 5 < n && is_word_char(chars[idx + 5].1) {
                (true, idx + 5)
            } else {
                (false, idx)
            };
        let mut end_idx = name_start_idx;
        while end_idx < n && is_word_char(chars[end_idx].1) {
            end_idx += 1;
        }
        let end_byte = if end_idx < n {
            chars[end_idx].0
        } else {
            text.len()
        };
        let name_start_byte = chars[name_start_idx].0;
        let name = &text[name_start_byte..end_byte];
        let followed_by_paren = text[end_byte..].starts_with('(');

        results.push_str(&call_escape(interp, &text[here_byte..byte_pos])?);
        if selfdot {
            if followed_by_paren {
                let linked = htmldoc_namelink_impl(interp, name, std::slice::from_ref(&methods))?;
                results.push_str("self.");
                results.push_str(&linked);
            } else {
                results.push_str(&format!("self.<strong>{name}</strong>"));
            }
        } else if followed_by_paren {
            let linked = htmldoc_namelink_impl(
                interp,
                name,
                &[methods.clone(), funcs.clone(), classes.clone()],
            )?;
            results.push_str(&linked);
        } else {
            let linked = htmldoc_namelink_impl(interp, name, std::slice::from_ref(&classes))?;
            results.push_str(&linked);
        }
        here_byte = end_byte;
        idx = end_idx;
    }

    results.push_str(&call_escape(interp, &text[here_byte..])?);
    Ok(results)
}

fn htmldoc_formattree_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    tree: &Value,
    modname: &Value,
    parent: &Value,
) -> Result<String> {
    let mut result = String::new();
    let mut last_c = Value::none();
    for entry in interp.collect_iterable(tree)? {
        if is_tuple_val(interp, &entry) {
            let parts = interp.collect_iterable(&entry)?;
            if parts.len() == 2 {
                let c = parts[0].clone();
                let bases = parts[1].clone();
                result.push_str("<dt class=\"heading-text\">");
                let clink =
                    call_method_str(interp, self_val, "classlink", &[c.clone(), modname.clone()])?;
                result.push_str(&clink);
                let bases_items = interp.collect_iterable(&bases)?;
                let same_as_parent =
                    bases_items.len() == 1 && values_are_identical(&bases_items[0], parent);
                if !bases_items.is_empty() && !same_as_parent {
                    let mut parents = Vec::with_capacity(bases_items.len());
                    for base in &bases_items {
                        let pl = call_method_str(
                            interp,
                            self_val,
                            "classlink",
                            &[base.clone(), modname.clone()],
                        )?;
                        parents.push(pl);
                    }
                    result.push('(');
                    result.push_str(&parents.join(", "));
                    result.push(')');
                }
                result.push_str("\n</dt>");
                last_c = c;
            }
        } else if is_list_val(interp, &entry) {
            let sub = call_method_str(
                interp,
                self_val,
                "formattree",
                &[entry, modname.clone(), last_c.clone()],
            )?;
            result.push_str(&format!("<dd>\n{sub}</dd>\n"));
        }
    }
    Ok(format!("<dl>\n{result}</dl>\n"))
}

fn htmldoc_docmodule_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    obj: &Value,
) -> Result<String> {
    let name_val = interp.get_attr(obj, "__name__")?;
    let name = py_str(interp, &name_val)?;
    let all = py_getattr_opt(interp, obj, "__all__");
    let parts: Vec<&str> = name.split('.').collect();
    let mut linked_parts: Vec<String> = Vec::with_capacity(parts.len());
    for i in 0..parts.len().saturating_sub(1) {
        let prefix = parts[..=i].join(".");
        linked_parts.push(format!(
            "<a href=\"{prefix}.html\" class=\"white\">{}</a>",
            parts[i]
        ));
    }
    if let Some(last) = parts.last() {
        linked_parts.push((*last).to_string());
    }
    let linkedname = linked_parts.join(".");
    let mut head = format!("<strong class=\"title\">{linkedname}</strong>");

    let filelink = match inspect_attr(interp, "getabsfile")
        .and_then(|f| call_pos(interp, f, std::slice::from_ref(obj)))
    {
        Ok(path_v) => {
            let path = py_str(interp, &path_v)?;
            let url = interp
                .load_module("urllib.parse")
                .and_then(|u| interp.get_attr(&u, "quote"))
                .and_then(|q| call_pos(interp, q, std::slice::from_ref(&path_v)))
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_else(|| path.clone());
            call_method_str(
                interp,
                self_val,
                "filelink",
                &[Value::string(url), Value::string(path)],
            )?
        }
        Err(ref e) if is_exc_err(interp, e, "TypeError") => "(built-in)".to_string(),
        Err(e) => return Err(e),
    };

    let mut info: Vec<String> = Vec::new();
    if let Some(ver) = py_getattr_opt(interp, obj, "__version__") {
        let v_str = format_version_str(&py_str(interp, &ver)?);
        let esc = call_method_str(interp, self_val, "escape", &[Value::string(v_str)])?;
        info.push(format!("version {esc}"));
    }
    if let Some(d) = py_getattr_opt(interp, obj, "__date__") {
        let s = py_str(interp, &d)?;
        let esc = call_method_str(interp, self_val, "escape", &[Value::string(s)])?;
        info.push(esc);
    }
    if !info.is_empty() {
        head.push_str(&format!(" ({})", info.join(", ")));
    }

    let docloc_val = call_method(interp, self_val, "getdocloc", std::slice::from_ref(obj))?;
    let docloc = if !docloc_val.is_none() {
        let dl = py_str(interp, &docloc_val)?;
        format!("<br><a href=\"{dl}\">Module Reference</a>")
    } else {
        String::new()
    };

    let mut result = call_method_str(
        interp,
        self_val,
        "heading",
        &[
            Value::string(head),
            Value::string(format!("<a href=\".\">index</a><br>{filelink}{docloc}")),
        ],
    )?;

    let getmembers = inspect_attr(interp, "getmembers")?;
    let getmodule = inspect_attr(interp, "getmodule")?;

    let ismodule_fn = inspect_attr(interp, "ismodule")?;
    let modules_val = call_pos(interp, getmembers.clone(), &[obj.clone(), ismodule_fn])?;
    let modules_items = interp.collect_iterable(&modules_val)?;

    let mut classes: Vec<(String, Value)> = Vec::new();
    let cdict = Value::dict(PyDict::default());
    let isclass_fn = inspect_attr(interp, "isclass")?;
    let class_members = call_pos(interp, getmembers.clone(), &[obj.clone(), isclass_fn])?;
    for pair in interp.collect_iterable(&class_members)? {
        let items = interp.collect_iterable(&pair)?;
        if items.len() != 2 {
            continue;
        }
        let key = py_str(interp, &items[0])?;
        let val = items[1].clone();
        let include_mod = if all.is_some() {
            true
        } else {
            let m = call_pos(interp, getmodule.clone(), std::slice::from_ref(&val))?;
            m.is_none() || values_are_identical(&m, obj)
        };
        if include_mod {
            let vis = visiblename_impl(interp, &key, all.as_ref(), Some(obj))?;
            if interp.truthy_value(&vis)? {
                let anchor = Value::string(format!("#{key}"));
                let _ = call_method(
                    interp,
                    &cdict,
                    "__setitem__",
                    &[Value::string(&key), anchor.clone()],
                );
                let _ = call_method(interp, &cdict, "__setitem__", &[val.clone(), anchor]);
                classes.push((key, val));
            }
        }
    }

    let sys_mod = interp.load_module("sys")?;
    let sys_modules = interp.get_attr(&sys_mod, "modules")?;
    for (_, val) in &classes {
        if let Ok(bases) = interp.get_attr(val, "__bases__") {
            for base in interp.collect_iterable(&bases)? {
                if let (Ok(bk_v), Ok(bm_v)) = (
                    interp.get_attr(&base, "__name__"),
                    interp.get_attr(&base, "__module__"),
                ) {
                    let bk = py_str(interp, &bk_v)?;
                    let bm = py_str(interp, &bm_v)?;
                    if bm != name
                        && let Ok(bmod) = call_method(interp, &sys_modules, "get", &[bm_v])
                        && !bmod.is_none()
                        && let Ok(attr) = interp.get_attr(&bmod, &bk)
                        && values_are_identical(&attr, &base)
                    {
                        let in_cd = interp.eval_in(cdict.clone(), Value::string(&bk))?;
                        if !interp.truthy_value(&in_cd)? {
                            let link = Value::string(format!("{bm}.html#{bk}"));
                            let _ = call_method(
                                interp,
                                &cdict,
                                "__setitem__",
                                &[Value::string(&bk), link.clone()],
                            );
                            let _ = call_method(interp, &cdict, "__setitem__", &[base, link]);
                        }
                    }
                }
            }
        }
    }

    let mut funcs: Vec<(String, Value)> = Vec::new();
    let fdict = Value::dict(PyDict::default());
    let isroutine_fn = inspect_attr(interp, "isroutine")?;
    let func_members = call_pos(interp, getmembers.clone(), &[obj.clone(), isroutine_fn])?;
    for pair in interp.collect_iterable(&func_members)? {
        let items = interp.collect_iterable(&pair)?;
        if items.len() != 2 {
            continue;
        }
        let key = py_str(interp, &items[0])?;
        let val = items[1].clone();
        let include_mod = if all.is_some() || inspect_pred(interp, "isbuiltin", &val)? {
            true
        } else {
            let m = call_pos(interp, getmodule.clone(), std::slice::from_ref(&val))?;
            values_are_identical(&m, obj)
        };
        if include_mod {
            let vis = visiblename_impl(interp, &key, all.as_ref(), Some(obj))?;
            if interp.truthy_value(&vis)? {
                let anchor = Value::string(format!("#-{key}"));
                let _ = call_method(
                    interp,
                    &fdict,
                    "__setitem__",
                    &[Value::string(&key), anchor.clone()],
                );
                if inspect_pred(interp, "isfunction", &val)? {
                    let _ = call_method(interp, &fdict, "__setitem__", &[val.clone(), anchor]);
                }
                funcs.push((key, val));
            }
        }
    }

    let mut data: Vec<(String, Value)> = Vec::new();
    let isdata_fn = pydoc_attr(interp, "isdata")?;
    let data_members = call_pos(interp, getmembers, &[obj.clone(), isdata_fn])?;
    for pair in interp.collect_iterable(&data_members)? {
        let items = interp.collect_iterable(&pair)?;
        if items.len() != 2 {
            continue;
        }
        let key = py_str(interp, &items[0])?;
        let val = items[1].clone();
        let vis = visiblename_impl(interp, &key, all.as_ref(), Some(obj))?;
        if interp.truthy_value(&vis)? {
            data.push((key, val));
        }
    }

    let preformat_fn = interp.get_attr(self_val, "preformat")?;
    let raw_doc = getdoc_impl(interp, obj)?;
    let marked_doc = call_method_str(
        interp,
        self_val,
        "markup",
        &[
            Value::string(&raw_doc),
            preformat_fn.clone(),
            fdict.clone(),
            cdict.clone(),
        ],
    )?;
    let doc_span = if marked_doc.is_empty() {
        String::new()
    } else {
        format!("<span class=\"code\">{marked_doc}</span>")
    };
    result.push_str(&format!("<p>{doc_span}</p>\n"));

    if let Some(pkg_path) = py_getattr_opt(interp, obj, "__path__") {
        let pkgutil = interp.load_module("pkgutil")?;
        let iter_modules = interp.get_attr(&pkgutil, "iter_modules")?;
        let iter = call_pos(interp, iter_modules, &[pkg_path])?;
        let mut modpkgs: Vec<(String, bool)> = Vec::new();
        for entry in interp.collect_iterable(&iter)? {
            let parts = interp.collect_iterable(&entry)?;
            if parts.len() >= 3 {
                let mname = py_str(interp, &parts[1])?;
                let ispkg = interp.truthy_value(&parts[2])?;
                modpkgs.push((mname, ispkg));
            }
        }
        modpkgs.sort_by(|a, b| a.0.cmp(&b.0));
        let modpkg_vals: Vec<Value> = modpkgs
            .into_iter()
            .map(|(m, ispkg)| {
                Value::tuple(vec![
                    Value::string(m),
                    name_val.clone(),
                    Value::bool_(ispkg),
                    Value::int(0),
                ])
            })
            .collect();
        let modpkglink = interp.get_attr(self_val, "modpkglink")?;
        let contents = htmldoc_multicolumn_impl(interp, &modpkg_vals, &modpkglink)?;
        let sec = call_method_str(
            interp,
            self_val,
            "bigsection",
            &[
                Value::string("Package Contents"),
                Value::string("pkg-content"),
                Value::string(contents),
            ],
        )?;
        result.push_str(&sec);
    } else if !modules_items.is_empty() {
        let mut rendered = String::new();
        let rows = modules_items.len().div_ceil(4);
        for col in 0..4 {
            rendered.push_str("<td class=\"multicolumn\">");
            for i in (rows * col)..(rows * col + rows) {
                if let Some(pair) = modules_items.get(i) {
                    let parts = interp.collect_iterable(pair)?;
                    if let Some(mod_obj) = parts.get(1) {
                        let mlink = call_method_str(
                            interp,
                            self_val,
                            "modulelink",
                            std::slice::from_ref(mod_obj),
                        )?;
                        rendered.push_str(&mlink);
                        rendered.push_str("<br>\n");
                    }
                }
            }
            rendered.push_str("</td>");
        }
        let contents = format!("<table><tr>{rendered}</tr></table>");
        let sec = call_method_str(
            interp,
            self_val,
            "bigsection",
            &[
                Value::string("Modules"),
                Value::string("pkg-content"),
                Value::string(contents),
            ],
        )?;
        result.push_str(&sec);
    }

    if !classes.is_empty() {
        let classlist = Value::list(classes.iter().map(|(_, v)| v.clone()).collect());
        let getclasstree = inspect_attr(interp, "getclasstree")?;
        let tree = call_pos(interp, getclasstree, &[classlist, Value::int(1)])?;
        let tree_str = call_method_str(interp, self_val, "formattree", &[tree, name_val.clone()])?;
        let mut contents = vec![tree_str];
        for (key, val) in &classes {
            let doc = call_method(
                interp,
                self_val,
                "document",
                &[
                    val.clone(),
                    Value::string(key),
                    name_val.clone(),
                    fdict.clone(),
                    cdict.clone(),
                ],
            )?;
            contents.push(py_str(interp, &doc)?);
        }
        let sec = call_method_str(
            interp,
            self_val,
            "bigsection",
            &[
                Value::string("Classes"),
                Value::string("index"),
                Value::string(contents.join(" ")),
            ],
        )?;
        result.push_str(&sec);
    }

    if !funcs.is_empty() {
        let mut contents = Vec::with_capacity(funcs.len());
        for (key, val) in &funcs {
            let doc = call_method(
                interp,
                self_val,
                "document",
                &[
                    val.clone(),
                    Value::string(key),
                    name_val.clone(),
                    fdict.clone(),
                    cdict.clone(),
                ],
            )?;
            contents.push(py_str(interp, &doc)?);
        }
        let sec = call_method_str(
            interp,
            self_val,
            "bigsection",
            &[
                Value::string("Functions"),
                Value::string("functions"),
                Value::string(contents.join(" ")),
            ],
        )?;
        result.push_str(&sec);
    }

    if !data.is_empty() {
        let mut contents = Vec::with_capacity(data.len());
        for (key, val) in &data {
            let doc = call_method(
                interp,
                self_val,
                "document",
                &[val.clone(), Value::string(key)],
            )?;
            contents.push(py_str(interp, &doc)?);
        }
        let sec = call_method_str(
            interp,
            self_val,
            "bigsection",
            &[
                Value::string("Data"),
                Value::string("data"),
                Value::string(contents.join("<br>\n")),
            ],
        )?;
        result.push_str(&sec);
    }

    if let Some(a) = py_getattr_opt(interp, obj, "__author__") {
        let s = py_str(interp, &a)?;
        let marked = call_method(
            interp,
            self_val,
            "markup",
            &[Value::string(s), preformat_fn.clone()],
        )?;
        let sec = call_method_str(
            interp,
            self_val,
            "bigsection",
            &[Value::string("Author"), Value::string("author"), marked],
        )?;
        result.push_str(&sec);
    }
    if let Some(c) = py_getattr_opt(interp, obj, "__credits__") {
        let s = py_str(interp, &c)?;
        let marked = call_method(
            interp,
            self_val,
            "markup",
            &[Value::string(s), preformat_fn],
        )?;
        let sec = call_method_str(
            interp,
            self_val,
            "bigsection",
            &[Value::string("Credits"), Value::string("credits"), marked],
        )?;
        result.push_str(&sec);
    }

    Ok(result)
}

fn htmldoc_docclass_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    obj: &Value,
    name_opt: Option<&Value>,
    mod_val: Value,
    funcs: Value,
    classes: Value,
) -> Result<String> {
    let realname = attr_str(interp, obj, "__name__")?;
    let name = match name_opt {
        Some(n) if interp.truthy_value(n)? => py_str(interp, n)?,
        _ => realname.clone(),
    };
    let bases = interp.get_attr(obj, "__bases__")?;
    let obj_mod = interp.get_attr(obj, "__module__")?;

    let mut contents: Vec<String> = Vec::new();
    let mut need_hr = false;
    let maybe_hr = |need: &mut bool, contents: &mut Vec<String>| {
        if *need {
            contents.push("<hr>\n".to_string());
        }
        *need = true;
    };

    let getmro = inspect_attr(interp, "getmro")?;
    let mro_val = call_pos(interp, getmro, std::slice::from_ref(obj))?;
    let mut mro: std::collections::VecDeque<Value> = interp.collect_iterable(&mro_val)?.into();
    if mro.len() > 2 {
        maybe_hr(&mut need_hr, &mut contents);
        contents.push("<dl><dt>Method resolution order:</dt>\n".to_string());
        for base in &mro {
            let link = call_method_str(
                interp,
                self_val,
                "classlink",
                &[base.clone(), obj_mod.clone()],
            )?;
            contents.push(format!("<dd>{link}</dd>\n"));
        }
        contents.push("</dl>\n".to_string());
    }

    let all_attrs = classify_class_attrs_impl(interp, obj)?;
    let mut attrs: Vec<(String, String, Value, Value)> = Vec::new();
    for entry in all_attrs {
        let vis = visiblename_impl(interp, &entry.0, None, Some(obj))?;
        if interp.truthy_value(&vis)? {
            attrs.push(entry);
        }
    }

    let mdict = Value::dict(PyDict::default());
    for (key, _kind, _homecls, _value) in &attrs {
        let anchor = Value::string(format!("#{name}-{key}"));
        let _ = call_method(
            interp,
            &mdict,
            "__setitem__",
            &[Value::string(key), anchor.clone()],
        );
        if let Ok(v) = interp.get_attr(obj, key) {
            let _ = call_method(interp, &mdict, "__setitem__", &[v, anchor]);
        }
    }

    let preformat_fn = interp.get_attr(self_val, "preformat")?;
    let builtins_object = Value::py_class(object_class_singleton());

    while !attrs.is_empty() {
        let thisclass = mro.pop_front().unwrap_or_else(|| attrs[0].2.clone());
        let (mut cur, inherited): (Vec<_>, Vec<_>) = attrs
            .into_iter()
            .partition(|t| values_are_identical(&t.2, &thisclass));

        if !values_are_identical(obj, &builtins_object)
            && values_are_identical(&thisclass, &builtins_object)
        {
            attrs = inherited;
            continue;
        }
        let tag = if values_are_identical(&thisclass, obj) {
            "defined here:<br>\n".to_string()
        } else {
            let link = call_method_str(
                interp,
                self_val,
                "classlink",
                &[thisclass.clone(), obj_mod.clone()],
            )?;
            format!("inherited from {link}:<br>\n")
        };

        sort_attributes_vec(interp, &mut cur, obj);

        for (heading_prefix, kind_match) in [
            ("Methods", "method"),
            ("Class methods", "class method"),
            ("Static methods", "static method"),
        ] {
            let (ok, rest): (Vec<_>, Vec<_>) = cur.into_iter().partition(|t| t.1 == kind_match);
            cur = rest;
            if !ok.is_empty() {
                maybe_hr(&mut need_hr, &mut contents);
                contents.push(format!("{heading_prefix} {tag}"));
                for (aname, _kind, homecls, value) in ok {
                    let rendered = match interp.get_attr(obj, &aname) {
                        Ok(val) => call_method(
                            interp,
                            self_val,
                            "document",
                            &[
                                val,
                                Value::string(&aname),
                                mod_val.clone(),
                                funcs.clone(),
                                classes.clone(),
                                mdict.clone(),
                                obj.clone(),
                                homecls,
                            ],
                        )?,
                        Err(_) => call_method(
                            interp,
                            self_val,
                            "docdata",
                            &[value, Value::string(&aname), mod_val.clone()],
                        )?,
                    };
                    contents.push(py_str(interp, &rendered)?);
                    contents.push("\n".to_string());
                }
            }
        }

        for (heading_prefix, kind_match) in [
            ("Readonly properties", "readonly property"),
            ("Data descriptors", "data descriptor"),
        ] {
            let (ok, rest): (Vec<_>, Vec<_>) = cur.into_iter().partition(|t| t.1 == kind_match);
            cur = rest;
            if !ok.is_empty() {
                maybe_hr(&mut need_hr, &mut contents);
                contents.push(format!("{heading_prefix} {tag}"));
                for (aname, _kind, _homecls, value) in ok {
                    let rendered = call_method(
                        interp,
                        self_val,
                        "docdata",
                        &[value, Value::string(&aname), mod_val.clone()],
                    )?;
                    contents.push(py_str(interp, &rendered)?);
                }
            }
        }

        let (ok, _rest): (Vec<_>, Vec<_>) = cur.into_iter().partition(|t| t.1 == "data");
        if !ok.is_empty() {
            maybe_hr(&mut need_hr, &mut contents);
            contents.push(format!("Data and other attributes {tag}"));
            for (aname, _kind, _homecls, value) in ok {
                let item_obj = interp
                    .get_attr(obj, &aname)
                    .unwrap_or_else(|_| value.clone());
                let base = call_method_str(
                    interp,
                    self_val,
                    "docother",
                    &[item_obj, Value::string(&aname), mod_val.clone()],
                )?;
                let doc = getdoc_impl(interp, &value)?;
                if doc.is_empty() {
                    contents.push(format!("<dl><dt>{base}</dl>\n"));
                } else {
                    let marked = call_method_str(
                        interp,
                        self_val,
                        "markup",
                        &[
                            Value::string(&doc),
                            preformat_fn.clone(),
                            funcs.clone(),
                            classes.clone(),
                            mdict.clone(),
                        ],
                    )?;
                    contents.push(format!(
                        "<dl><dt>{base}<dd><span class=\"code\">{marked}</span></dl>\n"
                    ));
                }
                contents.push("\n".to_string());
            }
        }

        attrs = inherited;
    }

    let body = contents.join("");
    let mut title = if name == realname {
        format!("<a name=\"{name}\">class <strong>{realname}</strong></a>")
    } else {
        format!("<strong>{name}</strong> = <a name=\"{name}\">class {realname}</a>")
    };
    let bases_items = interp.collect_iterable(&bases)?;
    if !bases_items.is_empty() {
        let mut parents = Vec::with_capacity(bases_items.len());
        for base in &bases_items {
            let link = call_method_str(
                interp,
                self_val,
                "classlink",
                &[base.clone(), obj_mod.clone()],
            )?;
            parents.push(link);
        }
        title.push_str(&format!("({})", parents.join(", ")));
    }

    let mut decl = String::new();
    let sig_fn = inspect_attr(interp, "signature")?;
    if let Ok(sig) = call_pos(interp, sig_fn, std::slice::from_ref(obj)) {
        let argspec = py_str(interp, &sig)?;
        if !argspec.is_empty() && argspec != "()" {
            let esc = call_method_str(interp, self_val, "escape", &[Value::string(&argspec)])?;
            decl = format!("{name}{esc}\n\n");
        }
    }

    let mut doc = getdoc_impl(interp, obj)?;
    if !decl.is_empty() {
        doc = format!("{decl}{doc}");
    }
    let marked_doc = call_method_str(
        interp,
        self_val,
        "markup",
        &[Value::string(&doc), preformat_fn, funcs, classes, mdict],
    )?;
    let prelude = if marked_doc.is_empty() {
        String::new()
    } else {
        format!("<span class=\"code\">{marked_doc}<br>&nbsp;</span>")
    };

    let sec = call_method(
        interp,
        self_val,
        "section",
        &[
            Value::string(title),
            Value::string("title"),
            Value::string(body),
            Value::int(3),
            Value::string(prelude),
        ],
    )?;
    py_str(interp, &sec)
}

#[allow(clippy::too_many_arguments)]
fn htmldoc_docroutine_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    obj: &Value,
    name_opt: Option<&Value>,
    mod_val: &Value,
    funcs: Value,
    classes: Value,
    methods: Value,
    cl: Option<&Value>,
    homecls_opt: Option<&Value>,
) -> Result<String> {
    let realname = attr_str(interp, obj, "__name__")?;
    let name = match name_opt {
        Some(n) if interp.truthy_value(n)? => py_str(interp, n)?,
        _ => realname.clone(),
    };
    let cl_opt = cl.filter(|v| !v.is_none());
    let homecls = homecls_opt.filter(|v| !v.is_none()).or(cl_opt);
    let cl_name = match cl_opt {
        Some(c) => attr_str(interp, c, "__name__")?,
        None => String::new(),
    };
    let anchor = format!("{cl_name}-{name}");
    let mut note = String::new();
    let mut skipdocs = false;
    let mut imfunc: Option<Value> = None;

    if is_bound_method_impl(interp, obj)? {
        let imself = interp.get_attr(obj, "__self__")?;
        if cl_opt.is_some_and(|c| values_are_identical(&imself, c)) {
            imfunc = py_getattr_opt(interp, obj, "__func__");
        } else if inspect_pred(interp, "isclass", &imself)? {
            let link = call_method_str(interp, self_val, "classlink", &[imself, mod_val.clone()])?;
            note = format!(" class method of {link}");
        } else {
            let imcls = interp.get_attr(&imself, "__class__")?;
            let link = call_method_str(interp, self_val, "classlink", &[imcls, mod_val.clone()])?;
            note = format!(" method of {link} instance");
        }
    } else if inspect_pred(interp, "ismethoddescriptor", obj)?
        || inspect_pred(interp, "ismethodwrapper", obj)?
    {
        if let Some(objclass) = py_getattr_opt(interp, obj, "__objclass__") {
            let link = call_method_str(
                interp,
                self_val,
                "classlink",
                &[objclass.clone(), mod_val.clone()],
            )?;
            if cl_opt.is_none() {
                note = format!(" unbound {link} method");
            } else if homecls.is_none_or(|hc| !values_are_identical(&objclass, hc)) {
                note = format!(" from {link}");
            }
        }
    } else {
        imfunc = Some(obj.clone());
    }

    if let Some(ref f) = imfunc
        && inspect_pred(interp, "isfunction", f)?
        && let Some(hc) = homecls
    {
        let f_mod =
            py_getattr_opt(interp, f, "__module__").and_then(|v| v.as_str().map(str::to_owned));
        let hc_mod =
            py_getattr_opt(interp, hc, "__module__").and_then(|v| v.as_str().map(str::to_owned));
        let f_qual =
            py_getattr_opt(interp, f, "__qualname__").and_then(|v| v.as_str().map(str::to_owned));
        let hc_qual =
            py_getattr_opt(interp, hc, "__qualname__").and_then(|v| v.as_str().map(str::to_owned));
        if f_mod != hc_mod
            || f_qual.as_deref()
                != hc_qual
                    .as_deref()
                    .map(|q| format!("{q}.{realname}"))
                    .as_deref()
        {
            let pname = call_method(
                interp,
                self_val,
                "parentlink",
                &[f.clone(), mod_val.clone()],
            )?;
            if interp.truthy_value(&pname)? {
                note = format!(" from {}", py_str(interp, &pname)?);
            }
        }
    }

    let asyncqualifier = if inspect_pred(interp, "iscoroutinefunction", obj)?
        || inspect_pred(interp, "isasyncgenfunction", obj)?
    {
        "async "
    } else {
        ""
    };

    let mut title = if name == realname {
        format!("<a name=\"{anchor}\"><strong>{realname}</strong></a>")
    } else {
        let reallink = if let Some(c) = cl_opt
            && let Ok(gs) = inspect_attr(interp, "getattr_static")
            && let Ok(static_val) = call_pos(
                interp,
                gs,
                &[c.clone(), Value::string(&realname), Value::list(Vec::new())],
            )
            && values_are_identical(&static_val, obj)
        {
            skipdocs = true;
            if note.starts_with(" from ") {
                note.clear();
            }
            format!("<a href=\"#{cl_name}-{realname}\">{realname}</a>")
        } else {
            realname.clone()
        };
        format!("<a name=\"{anchor}\"><strong>{name}</strong></a> = {reallink}")
    };

    let mut argspec: Option<String> = None;
    if inspect_pred(interp, "isroutine", obj)?
        && let Ok(sig_fn) = inspect_attr(interp, "signature")
        && let Ok(sig) = call_pos(interp, sig_fn, std::slice::from_ref(obj))
    {
        let mut s = py_str(interp, &sig)?;
        if realname == "<lambda>" {
            title = format!("<strong>{name}</strong> <em>lambda</em> ");
            let has_ann = py_getattr_opt(interp, obj, "__annotations__")
                .and_then(|v| interp.truthy_value(&v).ok())
                .unwrap_or(false);
            if !has_ann && s.len() >= 2 && s.starts_with('(') && s.ends_with(')') {
                s = s[1..s.len() - 1].to_string();
            }
        }
        if !s.is_empty() {
            argspec = Some(s);
        }
    }
    let argspec = argspec.unwrap_or_else(|| "(...)".to_string());
    let esc_argspec = call_method_str(interp, self_val, "escape", &[Value::string(&argspec)])?;
    let note_html = if note.is_empty() {
        String::new()
    } else {
        call_method_str(
            interp,
            self_val,
            "grey",
            &[Value::string(format!(
                "<span class=\"heading-text\">{note}</span>"
            ))],
        )?
    };
    let decl = format!("{asyncqualifier}{title}{esc_argspec}{note_html}");

    if skipdocs {
        Ok(format!("<dl><dt>{decl}</dt></dl>\n"))
    } else {
        let preformat_fn = interp.get_attr(self_val, "preformat")?;
        let raw_doc = getdoc_impl(interp, obj)?;
        let marked = call_method_str(
            interp,
            self_val,
            "markup",
            &[
                Value::string(&raw_doc),
                preformat_fn,
                funcs,
                classes,
                methods,
            ],
        )?;
        let doc_dd = if marked.is_empty() {
            String::new()
        } else {
            format!("<dd><span class=\"code\">{marked}</span></dd>")
        };
        Ok(format!("<dl><dt>{decl}</dt>{doc_dd}</dl>\n"))
    }
}

fn htmldoc_docdata_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    obj: &Value,
    name_opt: Option<&Value>,
) -> Result<String> {
    let mut out = String::new();
    if let Some(n) = name_opt
        && interp.truthy_value(n)?
    {
        let ns = py_str(interp, n)?;
        out.push_str(&format!("<dl><dt><strong>{ns}</strong></dt>\n"));
    }
    let preformat_fn = interp.get_attr(self_val, "preformat")?;
    let raw_doc = getdoc_impl(interp, obj)?;
    let marked = call_method_str(
        interp,
        self_val,
        "markup",
        &[Value::string(&raw_doc), preformat_fn],
    )?;
    if !marked.is_empty() {
        out.push_str(&format!("<dd><span class=\"code\">{marked}</span></dd>\n"));
    }
    out.push_str("</dl>\n");
    Ok(out)
}

fn htmldoc_docother_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    obj: &Value,
    name_opt: Option<&Value>,
) -> Result<String> {
    let repr_s = call_method_str(interp, self_val, "repr", std::slice::from_ref(obj))?;
    match name_opt {
        Some(n) if interp.truthy_value(n)? => {
            let ns = py_str(interp, n)?;
            Ok(format!("<strong>{ns}</strong> = {repr_s}"))
        }
        _ => Ok(repr_s),
    }
}

fn htmldoc_index_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    dir: &Value,
    shadowed_opt: Option<Value>,
) -> Result<String> {
    let shadowed = match shadowed_opt.filter(|v| !v.is_none()) {
        Some(v) => v,
        None => Value::dict(PyDict::default()),
    };
    let pkgutil = interp.load_module("pkgutil")?;
    let iter_modules = interp.get_attr(&pkgutil, "iter_modules")?;
    let iter = call_pos(interp, iter_modules, &[Value::list(vec![dir.clone()])])?;
    let mut modpkgs: Vec<(String, bool, bool)> = Vec::new();
    for entry in interp.collect_iterable(&iter)? {
        let parts = interp.collect_iterable(&entry)?;
        if parts.len() >= 3 {
            let name = py_str(interp, &parts[1])?;
            let ispkg = interp.truthy_value(&parts[2])?;
            let in_sh = interp.eval_in(shadowed.clone(), Value::string(&name))?;
            let is_shadowed = interp.truthy_value(&in_sh)?;
            modpkgs.push((name.clone(), ispkg, is_shadowed));
            let _ = call_method(
                interp,
                &shadowed,
                "__setitem__",
                &[Value::string(&name), Value::int(1)],
            );
        }
    }
    modpkgs.sort_by(|a, b| a.0.cmp(&b.0));
    let modpkg_vals: Vec<Value> = modpkgs
        .into_iter()
        .map(|(n, ispkg, sh)| {
            Value::tuple(vec![
                Value::string(n),
                Value::string(""),
                Value::bool_(ispkg),
                Value::bool_(sh),
            ])
        })
        .collect();
    let modpkglink = interp.get_attr(self_val, "modpkglink")?;
    let contents = htmldoc_multicolumn_impl(interp, &modpkg_vals, &modpkglink)?;
    let sec = call_method(
        interp,
        self_val,
        "bigsection",
        &[dir.clone(), Value::string("index"), Value::string(contents)],
    )?;
    py_str(interp, &sec)
}

// ── Helper implementation ────────────────────────────────────────────────────

fn helper_write_output(interp: &mut Interpreter, self_val: &Value, text: &str) -> Result<()> {
    let out = interp.get_attr(self_val, "output")?;
    call_method(interp, &out, "write", &[Value::string(text)])?;
    Ok(())
}

fn helper_list_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    items_val: &Value,
    columns: usize,
    width: usize,
) -> Result<()> {
    let mut items: Vec<String> = Vec::new();
    for item in interp.collect_iterable(items_val)? {
        items.push(py_str(interp, &item)?);
    }
    items.sort();
    let cols = columns.max(1);
    let colw = width / cols;
    let rows = items.len().div_ceil(cols);
    let out = interp.get_attr(self_val, "output")?;
    let write_fn = interp.get_attr(&out, "write")?;
    for row in 0..rows {
        for col in 0..cols {
            let i = col * rows + row;
            if i < items.len() {
                call_pos(interp, write_fn.clone(), &[Value::string(&items[i])])?;
                if col < cols - 1 {
                    let pad = colw.saturating_sub(1 + items[i].chars().count());
                    let spaces = " ".repeat(1 + pad);
                    call_pos(interp, write_fn.clone(), &[Value::string(spaces)])?;
                }
            }
        }
        call_pos(interp, write_fn.clone(), &[Value::string("\n")])?;
    }
    Ok(())
}

fn helper_gettopic_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    topic: &str,
    more_xrefs: &str,
) -> Result<(String, String)> {
    let topics_mod = match interp.load_module("pydoc_data.topics") {
        Ok(m) => m,
        Err(_) => {
            return Ok((
                "\nSorry, topic and keyword documentation is not available because the\n\
module \"pydoc_data.topics\" could not be found.\n"
                    .to_string(),
                String::new(),
            ));
        }
    };
    let topics_dict = interp.get_attr(self_val, "topics")?;
    let keywords_dict = interp.get_attr(self_val, "keywords")?;

    let mut cur_topic = topic.to_string();
    for _ in 0..16 {
        let mut target = call_method(
            interp,
            &topics_dict,
            "get",
            &[Value::string(&cur_topic), Value::none()],
        )?;
        if target.is_none() {
            target = call_method(
                interp,
                &keywords_dict,
                "get",
                &[Value::string(&cur_topic), Value::none()],
            )?;
        }
        if !interp.truthy_value(&target)? {
            return Err(PyError::named("ValueError", "could not find topic"));
        }
        if let Some(alias) = target.as_str() {
            cur_topic = alias.to_string();
            continue;
        }
        let pair = interp.collect_iterable(&target)?;
        if pair.len() != 2 {
            return Err(PyError::named("ValueError", "could not find topic"));
        }
        let label = py_str(interp, &pair[0])?;
        let mut xrefs = py_str(interp, &pair[1])?;
        let data_topics = interp.get_attr(&topics_mod, "topics")?;
        let doc_val = call_method(
            interp,
            &data_topics,
            "__getitem__",
            &[Value::string(&label)],
        )?;
        let doc = py_str(interp, &doc_val)?;
        if !more_xrefs.is_empty() {
            if !xrefs.is_empty() {
                xrefs.push(' ');
            }
            xrefs.push_str(more_xrefs);
        }
        return Ok((doc, xrefs));
    }
    Err(PyError::named("ValueError", "could not find topic"))
}

fn helper_showtopic_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    topic: &str,
    more_xrefs: &str,
) -> Result<()> {
    let topics_mod = match interp.load_module("pydoc_data.topics") {
        Ok(m) => m,
        Err(_) => {
            helper_write_output(
                interp,
                self_val,
                "\nSorry, topic and keyword documentation is not available because the\n\
module \"pydoc_data.topics\" could not be found.\n",
            )?;
            return Ok(());
        }
    };
    let topics_dict = interp.get_attr(self_val, "topics")?;
    let keywords_dict = interp.get_attr(self_val, "keywords")?;

    let mut cur_topic = topic.to_string();
    for _ in 0..16 {
        let mut target = call_method(
            interp,
            &topics_dict,
            "get",
            &[Value::string(&cur_topic), Value::none()],
        )?;
        if target.is_none() {
            target = call_method(
                interp,
                &keywords_dict,
                "get",
                &[Value::string(&cur_topic), Value::none()],
            )?;
        }
        if !interp.truthy_value(&target)? {
            let r = py_repr(interp, &Value::string(&cur_topic))?;
            helper_write_output(
                interp,
                self_val,
                &format!("no documentation found for {r}\n"),
            )?;
            return Ok(());
        }
        if let Some(alias) = target.as_str() {
            cur_topic = alias.to_string();
            continue;
        }
        let pair = interp.collect_iterable(&target)?;
        if pair.len() != 2 {
            let r = py_repr(interp, &Value::string(&cur_topic))?;
            helper_write_output(
                interp,
                self_val,
                &format!("no documentation found for {r}\n"),
            )?;
            return Ok(());
        }
        let label = py_str(interp, &pair[0])?;
        let mut xrefs = py_str(interp, &pair[1])?;
        let data_topics = interp.get_attr(&topics_mod, "topics")?;
        let doc_val = match call_method(
            interp,
            &data_topics,
            "__getitem__",
            &[Value::string(&label)],
        ) {
            Ok(v) => v,
            Err(ref e) if is_exc_err(interp, e, "KeyError") => {
                let r = py_repr(interp, &Value::string(&cur_topic))?;
                helper_write_output(
                    interp,
                    self_val,
                    &format!("no documentation found for {r}\n"),
                )?;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let mut doc = format!("{}\n", py_str(interp, &doc_val)?.trim());
        if !more_xrefs.is_empty() {
            if !xrefs.is_empty() {
                xrefs.push(' ');
            }
            xrefs.push_str(more_xrefs);
        }
        if !xrefs.is_empty() {
            let joined = xrefs.split_whitespace().collect::<Vec<_>>().join(", ");
            let text = format!("Related help topics: {joined}\n");
            let textwrap = interp.load_module("textwrap")?;
            let wrap_fn = interp.get_attr(&textwrap, "wrap")?;
            let wrapped_val = call_pos(interp, wrap_fn, &[Value::string(text), Value::int(72)])?;
            let lines: Vec<String> = interp
                .collect_iterable(&wrapped_val)?
                .iter()
                .map(|v| py_str(interp, v))
                .collect::<Result<Vec<_>>>()?;
            doc.push_str(&format!("\n{}\n", lines.join("\n")));
        }
        let pager_fn = pydoc_attr(interp, "pager")?;
        call_pos(interp, pager_fn, &[Value::string(doc)])?;
        return Ok(());
    }
    Ok(())
}

fn helper_help_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    request: &Value,
    is_cli: bool,
) -> Result<()> {
    let self_output = py_getattr_opt(interp, self_val, "_output").unwrap_or_else(Value::none);
    let doc_fn = pydoc_attr(interp, "doc")?;
    let title_v = Value::string("Help on %s:");

    if let Some(raw_req) = request.as_str() {
        let req = raw_req.trim();
        let symbols = interp.get_attr(self_val, "symbols")?;
        let keywords = interp.get_attr(self_val, "keywords")?;
        let topics = interp.get_attr(self_val, "topics")?;
        let req_v = Value::string(req);

        let in_symbols = {
            let v = interp.eval_in(symbols, req_v.clone())?;
            interp.truthy_value(&v)?
        };
        let in_keywords = {
            let v = interp.eval_in(keywords, req_v.clone())?;
            interp.truthy_value(&v)?
        };
        let in_topics = {
            let v = interp.eval_in(topics, req_v.clone())?;
            interp.truthy_value(&v)?
        };

        if req == "keywords" {
            call_method(interp, self_val, "listkeywords", &[])?;
        } else if req == "symbols" {
            call_method(interp, self_val, "listsymbols", &[])?;
        } else if req == "topics" {
            call_method(interp, self_val, "listtopics", &[])?;
        } else if req == "modules" {
            call_method(interp, self_val, "listmodules", &[])?;
        } else if let Some(rest) = req.strip_prefix("modules ") {
            let key = rest.split_whitespace().next().unwrap_or("");
            call_method(interp, self_val, "listmodules", &[Value::string(key)])?;
        } else if in_symbols {
            call_method(interp, self_val, "showsymbol", &[req_v])?;
        } else if matches!(req, "True" | "False" | "None") {
            let val = match req {
                "True" => Value::bool_(true),
                "False" => Value::bool_(false),
                _ => Value::none(),
            };
            call_kw(
                interp,
                doc_fn,
                &[val, title_v],
                &[("is_cli", Value::bool_(is_cli))],
            )?;
        } else if in_keywords || in_topics {
            call_method(interp, self_val, "showtopic", &[req_v])?;
        } else if !req.is_empty() {
            call_kw(
                interp,
                doc_fn,
                &[req_v, title_v],
                &[("output", self_output), ("is_cli", Value::bool_(is_cli))],
            )?;
        } else {
            let str_cls = primitive_class_by_name("str")
                .map(Value::py_class)
                .unwrap_or_else(|| Value::builtin_function("str"));
            call_kw(
                interp,
                doc_fn,
                &[str_cls, title_v],
                &[("output", self_output), ("is_cli", Value::bool_(is_cli))],
            )?;
        }
    } else if let Ok(helper_cls) = pydoc_attr(interp, "Helper")
        && py_isinstance(interp, request, &helper_cls).unwrap_or(false)
    {
        call_pos(interp, self_val.clone(), &[])?;
    } else {
        call_kw(
            interp,
            doc_fn,
            &[request.clone(), title_v],
            &[("output", self_output), ("is_cli", Value::bool_(is_cli))],
        )?;
    }
    helper_write_output(interp, self_val, "\n")?;
    Ok(())
}

fn modulescanner_run_impl(
    interp: &mut Interpreter,
    self_val: &Value,
    callback: &Value,
    key_opt: Option<&Value>,
    completer_opt: Option<&Value>,
    onerror_opt: Option<&Value>,
) -> Result<()> {
    let key_lower = match key_opt {
        Some(v) if interp.truthy_value(v)? => Some(py_str(interp, v)?.to_lowercase()),
        _ => None,
    };
    interp.assign_attr(self_val.clone(), "quit", Value::bool_(false))?;

    let sys_mod = interp.load_module("sys")?;
    let builtin_names = interp.get_attr(&sys_mod, "builtin_module_names")?;
    for modname_v in interp.collect_iterable(&builtin_names)? {
        let modname = py_str(interp, &modname_v)?;
        if modname == "__main__" {
            continue;
        }
        match &key_lower {
            None => {
                call_pos(
                    interp,
                    callback.clone(),
                    &[Value::none(), modname_v, Value::string("")],
                )?;
            }
            Some(k) => {
                let doc_str = interp
                    .load_module(&modname)
                    .ok()
                    .and_then(|m| py_getattr_opt(interp, &m, "__doc__"))
                    .and_then(|d| d.as_str().map(str::to_owned))
                    .unwrap_or_default();
                let desc = doc_str.split('\n').next().unwrap_or("");
                let combined = format!("{modname} - {desc}");
                if combined.to_lowercase().contains(k) {
                    call_pos(
                        interp,
                        callback.clone(),
                        &[Value::none(), modname_v, Value::string(desc)],
                    )?;
                }
            }
        }
    }

    let pkgutil = interp.load_module("pkgutil")?;
    let walk_packages = interp.get_attr(&pkgutil, "walk_packages")?;
    let walk_iter = match onerror_opt.filter(|v| !v.is_none()) {
        Some(oe) => call_kw(interp, walk_packages, &[], &[("onerror", oe.clone())])?,
        None => call_pos(interp, walk_packages, &[])?,
    };

    for entry in interp.collect_iterable(&walk_iter)? {
        let quit_v =
            py_getattr_opt(interp, self_val, "quit").unwrap_or_else(|| Value::bool_(false));
        if interp.truthy_value(&quit_v)? {
            break;
        }
        let parts = interp.collect_iterable(&entry)?;
        if parts.len() < 3 {
            continue;
        }
        let importer = &parts[0];
        let modname_v = &parts[1];
        let modname = py_str(interp, modname_v)?;
        match &key_lower {
            None => {
                call_pos(
                    interp,
                    callback.clone(),
                    &[Value::none(), modname_v.clone(), Value::string("")],
                )?;
            }
            Some(k) => {
                let spec = match call_method(
                    interp,
                    importer,
                    "find_spec",
                    std::slice::from_ref(modname_v),
                ) {
                    Ok(s) if !s.is_none() => s,
                    _ => continue,
                };
                let Ok(loader) = interp.get_attr(&spec, "loader") else {
                    continue;
                };
                if loader.is_none() {
                    continue;
                }
                let (desc, path) = if py_hasattr(interp, &loader, "get_source") {
                    let source = match call_method(
                        interp,
                        &loader,
                        "get_source",
                        std::slice::from_ref(modname_v),
                    ) {
                        Ok(s) if s.as_str().is_some() => s,
                        Ok(_) => continue,
                        Err(_) => {
                            if let Some(oe) = onerror_opt.filter(|v| !v.is_none()) {
                                let _ =
                                    call_pos(interp, oe.clone(), std::slice::from_ref(modname_v));
                            }
                            continue;
                        }
                    };
                    let io_mod = interp.load_module("io")?;
                    let sio_cls = interp.get_attr(&io_mod, "StringIO")?;
                    let sio = call_pos(interp, sio_cls, &[source])?;
                    let syn = source_synopsis_impl(interp, &sio)?;
                    let desc = syn.as_str().unwrap_or("").to_string();
                    let path = if py_hasattr(interp, &loader, "get_filename") {
                        call_method(
                            interp,
                            &loader,
                            "get_filename",
                            std::slice::from_ref(modname_v),
                        )
                        .unwrap_or_else(|_| Value::none())
                    } else {
                        Value::none()
                    };
                    (desc, path)
                } else {
                    continue;
                };
                let combined = format!("{modname} - {desc}");
                if combined.to_lowercase().contains(k) {
                    call_pos(
                        interp,
                        callback.clone(),
                        &[path, modname_v.clone(), Value::string(desc)],
                    )?;
                }
            }
        }
    }

    if let Some(comp) = completer_opt
        && interp.truthy_value(comp)?
    {
        call_pos(interp, comp.clone(), &[])?;
    }
    Ok(())
}
