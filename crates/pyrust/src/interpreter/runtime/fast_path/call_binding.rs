impl Interpreter {
    /// Read the `npos` leading positionals, the `*args` splat elements (a plain
    /// tuple/list), and the `**kw` dict values (in iteration order, aligned with
    /// the cached `slots`) and bind them straight into the callee frame via the
    /// #2382 fast bind.  Caller guarantees the cache entry was a validated
    /// `ExArgs` hit for this `(callee, total_pos, key-set)`, so no per-call
    /// diagnostics are needed.
    #[allow(clippy::too_many_arguments)]
    fn call_ex_args_fast_bind(
        &mut self,
        f: &Rc<UserFunction>,
        regs: &RegSlice,
        func: crate::bytecode::Reg,
        npos: u8,
        args_splat_val: &Value,
        total_pos: usize,
        kwargs_val: Option<&Value>,
        slots: &[u32],
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let Some(callee_code) = self.get_or_compile_bytecode(f) else {
            return Err(PyError::Runtime(format!("no bytecode for '{}'", f.name)));
        };
        // Positionals, in source order: leading fixed positionals, then the splat
        // elements (a plain tuple/list — cloned, not the same Rc, so the callee's
        // `*args` keeps CPython's fresh-tuple identity when it is variadic; a
        // variadic callee never reaches this fast bind anyway).
        let mut pos_vals: smallvec::SmallVec<[Value; 8]> =
            smallvec::SmallVec::with_capacity(total_pos);
        for i in 0..npos as u32 {
            pos_vals.push(vm_read(regs, func + 1 + i, num_locals)?);
        }
        if let Some(s) = args_splat_val.as_tuple() {
            pos_vals.extend(s.iter().cloned());
        } else if let Some(items) = args_splat_val.list_with(|v| v.clone()) {
            pos_vals.extend(items);
        }
        // Dict values in iteration order (aligned with `slots` / the cached keyset).
        let kw_vals: smallvec::SmallVec<[Value; 4]> = kwargs_val
            .and_then(|v| v.dict_with(|d| d.values().cloned().collect()))
            .unwrap_or_default();
        self.call_user_function_kw_cached(
            f,
            &callee_code,
            total_pos,
            &mut pos_vals.into_iter(),
            slots,
            &mut kw_vals.into_iter(),
        )
    }

    /// Is `f` a PURE variadic-forward target — params exactly a single `*args`
    /// plus an optional `**kwargs` and nothing else?  That is the only shape the
    /// #2852 direct-bind path (`call_ex_args_pure_forward_bind`) handles: with no
    /// fixed positional / keyword-only / positional-only param, the callee's `*A`
    /// is just "all positionals given" and `**K` is just "all keywords given", so
    /// the tuple + dict can be built without the general per-param resolution.
    ///
    /// Also excludes callees that need the generic frame machinery in ways the
    /// direct path still routes through `bind_and_run_variadic_frame` correctly —
    /// generators / coroutines / cell / closure / global-using callees are all
    /// handled by that shared tail (identical to the generic split path), so they
    /// stay eligible; only the *param shape* is restrictive here.
    fn is_pure_variadic_forward(f: &Rc<UserFunction>) -> bool {
        let mut seen_args = false;
        let mut seen_kwargs = false;
        for p in &f.params {
            if p.is_args {
                if seen_args {
                    return false;
                }
                seen_args = true;
            } else if p.is_kwargs {
                if seen_kwargs {
                    return false;
                }
                seen_kwargs = true;
            } else {
                // Any fixed positional / keyword-only / positional-only param
                // disqualifies the shape.
                return false;
            }
        }
        seen_args
    }

    /// Direct-bind a positional-splat call into a PURE-VARIADIC-FORWARD callee —
    /// one whose params are exactly a single `*A` plus an optional `**K` and
    /// nothing else (`def inner(*A)` / `def inner(*A, **K)`).  For that shape the
    /// callee's `*A` is simply "every positional given" and `**K` is "every
    /// keyword given", so the tuple + dict are built DIRECTLY here and bound into
    /// the two param registers — skipping the `positional_vals` / `keyword_vals` /
    /// `param_vals` intermediate vectors and the whole per-param binding loop in
    /// `call_user_function_variadic_split`.
    ///
    /// Identity / freshness landmines (both required for CPython parity):
    /// - `*A` is a FRESH `Value::tuple` built from the elements; the caller's
    ///   splat `Rc` is never reused (`def f(*a): f(*t)` gives `a is not t`).
    /// - `**K` is a FRESH dict; a callee mutating `kwargs` must not touch the
    ///   caller's dict.
    ///
    /// Returns `Ok(None)` (→ caller slow path) when a `**kw` key is non-`str`
    /// (CPython "keywords must be strings"), or when there is no `**K` param but
    /// keywords were passed (CPython "unexpected keyword argument"): the general
    /// binder in `call_user_function_variadic_split` owns those diagnostics.
    #[allow(clippy::too_many_arguments)]
    fn call_ex_args_pure_forward_bind(
        &mut self,
        f: &Rc<UserFunction>,
        regs: &RegSlice,
        func: crate::bytecode::Reg,
        npos: u8,
        lit_kw: &[(String, Value)],
        args_splat_val: &Value,
        kwargs_val: Option<&Value>,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Option<Value>> {
        // Build the callee's `*A` tuple directly: leading positionals, then the
        // splat elements (a plain tuple/list — cloned, so the fresh-tuple identity
        // holds).  A single allocation, no intermediate positional Vec.
        let splat_len = args_splat_val
            .as_tuple()
            .map(|s| s.len())
            .or_else(|| args_splat_val.list_len())
            .unwrap_or(0);
        let mut args_elems: Vec<Value> = Vec::with_capacity(npos as usize + splat_len);
        for i in 0..npos as u32 {
            args_elems.push(vm_read(regs, func + 1 + i, num_locals)?);
        }
        if let Some(s) = args_splat_val.as_tuple() {
            args_elems.extend(s.iter().cloned());
        } else if let Some(items) = args_splat_val.list_with(|v| v.clone()) {
            args_elems.extend(items);
        }
        let args_tuple = Value::tuple(args_elems);

        // Does the callee have a `**K` param?  (The shape guarantees at most one.)
        let has_kwargs_param = f.params.iter().any(|p| p.is_kwargs);

        // Build the callee's `**K` dict directly from the keywords: literal `kw=v`
        // first, then the `**kw` dict entries (rejecting non-`str` keys → slow
        // path).  Literal keywords and `**kw` never co-occur (shape check).
        let kwargs_value: Value = if !has_kwargs_param {
            // No `**K`: any keyword is an error whose CPython diagnostic the general
            // binder owns → fall back.  A non-`str` `**kw` key is likewise a slow
            // path (the general binder still forwards, then raises there).
            if !lit_kw.is_empty() {
                return Ok(None);
            }
            if let Some(v) = kwargs_val {
                let non_empty = v.dict_with(|d| !d.is_empty()).unwrap_or(true);
                if non_empty {
                    return Ok(None);
                }
            }
            Value::unset()
        } else {
            let mut dict: PyDict = PyDict::default();
            for (k, v) in lit_kw {
                if let Some(key) = Value::string(k.clone()).to_key() {
                    dict.insert(key, v.clone());
                }
            }
            if let Some(v) = kwargs_val {
                // Reject a non-plain-dict or non-`str` key → slow path.
                let ok = match v.dict_with(|d| {
                    for (k, val) in d.iter() {
                        match k {
                            pyrust_core::PyKey::Str(_) => {
                                dict.insert(k.clone(), val.clone());
                            }
                            _ => return false,
                        }
                    }
                    true
                }) {
                    Some(true) => true,
                    // Non-str key (Some(false)) or not a plain dict (None): slow path.
                    _ => false,
                };
                if !ok {
                    return Ok(None);
                }
            }
            Value::dict(dict)
        };

        // Fill `param_vals` in param-index order on the STACK (no heap `Vec`).
        // Python syntax fixes the order for this shape: `*A` first, then the
        // optional `**K`, so `params` is exactly `[*A]` or `[*A, **K]`.  Each
        // value is moved into its register by the shared frame tail.
        let mut param_vals: smallvec::SmallVec<[Value; 2]> = smallvec::SmallVec::new();
        param_vals.push(args_tuple);
        if has_kwargs_param {
            param_vals.push(kwargs_value);
        }
        self.bind_and_run_variadic_frame(Rc::clone(f), &mut param_vals)
            .map(Some)
    }

    /// Forward a positional-splat call straight into a VARIADIC callee's binder,
    /// skipping the `ExpandedCallArg` buffer.  Builds `positional_vals` (leading
    /// positionals + `*args` splat elements, a plain tuple/list) and
    /// `keyword_vals` (the `**kw` dict entries) DIRECTLY and hands them to
    /// `call_user_function_variadic_split`, which owns every binding diagnostic
    /// (arity, missing, unexpected-keyword, `*args` absorption, `**kwargs`
    /// residual).  The callee's `*args` tuple is built fresh from the splat
    /// elements (the caller's `Rc` is never reused — CPython gives a distinct
    /// tuple, so `a is caller_tuple` stays `False`).
    ///
    /// Returns `Ok(None)` when the `**kw` dict has a non-`str` key — the caller
    /// then takes the slow path, which raises "keywords must be strings".
    #[allow(clippy::too_many_arguments)]
    fn call_ex_args_variadic_bind(
        &mut self,
        f: &Rc<UserFunction>,
        regs: &RegSlice,
        func: crate::bytecode::Reg,
        npos: u8,
        lit_kw: &[(String, Value)],
        args_splat_val: &Value,
        kwargs_val: Option<&Value>,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Option<Value>> {
        // Keyword entries as (name, value): the literal `kw=v` arguments first,
        // then the `**kw` dict entries (rejecting non-`str` keys → None, slow path
        // raises the CPython TypeError).  Literal keywords and `**kw` never co-occur
        // (shape check), so no cross-source collision is possible here.
        let mut keyword_vals: Vec<(String, Value)> = Vec::with_capacity(lit_kw.len());
        keyword_vals.extend(lit_kw.iter().cloned());
        if let Some(v) = kwargs_val {
            match v.dict_with(|d| {
                let mut kv: Vec<(String, Value)> = Vec::with_capacity(d.len());
                for (k, val) in d.iter() {
                    match k {
                        pyrust_core::PyKey::Str(s) => {
                            kv.push((s.as_str().unwrap_or("").to_owned(), val.clone()))
                        }
                        _ => return None,
                    }
                }
                Some(kv)
            }) {
                Some(Some(kv)) => keyword_vals.extend(kv),
                // Non-str key (Some(None)) or not a plain dict (None): slow path.
                _ => return Ok(None),
            }
        }

        // Positionals in source order: leading fixed positionals, then the splat
        // elements (plain tuple/list — cloned; the callee's `*args` tuple is built
        // fresh inside the binder, so no identity leak).
        let mut positional_vals: Vec<Value> = Vec::with_capacity(npos as usize + 4);
        for i in 0..npos as u32 {
            positional_vals.push(vm_read(regs, func + 1 + i, num_locals)?);
        }
        if let Some(s) = args_splat_val.as_tuple() {
            positional_vals.extend(s.iter().cloned());
        } else if let Some(items) = args_splat_val.list_with(|v| v.clone()) {
            positional_vals.extend(items);
        }

        let has_args_param = f.params.iter().any(|p| p.is_args);
        self.call_user_function_variadic_split(
            Rc::clone(f),
            positional_vals,
            keyword_vals,
            has_args_param,
        )
        .map(Some)
    }

    /// Read the `npos` positional registers and the `**d` dict values (in
    /// iteration order, aligned with the cached `slots`) and bind them straight
    /// into the callee frame via the #2382 fast bind.  Caller guarantees the
    /// cache entry was a validated `ExSimple` hit for this `(callee, npos,
    /// key-set)`, so no per-call diagnostics are needed.
    #[allow(clippy::too_many_arguments)]
    fn call_ex_fast_bind(
        &mut self,
        f: &Rc<UserFunction>,
        regs: &RegSlice,
        func: crate::bytecode::Reg,
        npos: usize,
        kwargs_val: &Value,
        slots: &[u32],
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let Some(callee_code) = self.get_or_compile_bytecode(f) else {
            return Err(PyError::Runtime(format!("no bytecode for '{}'", f.name)));
        };
        let mut pos_vals: smallvec::SmallVec<[Value; 4]> = smallvec::SmallVec::with_capacity(npos);
        for i in 0..npos as u32 {
            pos_vals.push(vm_read(regs, func + 1 + i, num_locals)?);
        }
        // Dict values in iteration order (aligned with `slots` / the cached keyset).
        let kw_vals: smallvec::SmallVec<[Value; 4]> = kwargs_val
            .dict_with(|d| d.values().cloned().collect())
            .unwrap_or_default();
        self.call_user_function_kw_cached(
            f,
            &callee_code,
            npos,
            &mut pos_vals.into_iter(),
            slots,
            &mut kw_vals.into_iter(),
        )
    }
}
