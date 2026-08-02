impl Interpreter {
    /// Full body of `Insn::CallEx` — a double-splat expansion call `f(<pos…>,
    /// **d)` (issue #2393).  Positionals occupy `R[func+1 .. func+1+npos]`;
    /// `R[kwargs]` holds the `**d` source mapping.  Returns the call result.
    ///
    /// Fast path: a plain `Regular` `UserFunction` callee and a plain `dict`
    /// splat.  A per-call-site shape cache ([`KwCallCacheEntry::ExSimple`]) keyed
    /// on `(param_binds identity, npos, dict key-set)` records the key→slot
    /// mapping; on a hit (same callee, same `npos`, same keys in order) the
    /// positional and dict values bind straight into their parameter slots via
    /// the #2382 `call_user_function_kw_cached`, with **no dict copy** and **no
    /// name scan**.  On a key-set miss the site re-resolves (the dict shape may
    /// have changed, e.g. value mutation never does but a different `**d` might);
    /// a structurally non-simple callee (has `**kwargs`, posonly-as-keyword, a
    /// duplicate, a missing required, etc.) pins the site to `Fallback`.
    ///
    /// Slow path: anything else — non-dict splat, dict subclass, non-`Regular`
    /// callee, non-`str` key, or a `Fallback`-pinned site — builds an
    /// `ExpandedCallArg` buffer and dispatches through `call_function_expanded`,
    /// which owns every CPython-parity binding diagnostic.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn exec_call_ex(
        &mut self,
        regs: &RegSlice,
        code: &crate::bytecode::FnCode,
        pc: usize,
        func: crate::bytecode::Reg,
        npos: u8,
        kwargs: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        use crate::bytecode::KwCallCacheEntry;
        let npos = npos as usize;
        let func_val = vm_read(regs, func, num_locals)?;
        let kwargs_val = vm_read(regs, kwargs, num_locals)?;

        // Fast path requires a plain user function (Regular kind) and a plain
        // `dict` splat (not a subclass, whose iteration may be overridden).
        let user_fn = match func_val.kind() {
            ValueKind::UserFunction(f)
                if matches!(f.kind, pyrust_core::UserFunctionKind::Regular) =>
            {
                Some(Rc::clone(f))
            }
            _ => None,
        };
        let is_plain_dict = matches!(kwargs_val.kind(), ValueKind::Dict(_));

        // #2395: bypass kw-call cache when __defaults__ has been overridden.
        if let (Some(f), true) = (
            user_fn.filter(|f| f.defaults_override.borrow().is_none()),
            is_plain_dict,
        ) {
            let pbptr = Rc::as_ptr(&f.param_binds);

            // Cache state: a hit reuses `slots`; a key-set/identity miss re-resolves
            // (unless pinned to `Fallback`).
            enum Action {
                Hit(smallvec::SmallVec<[u32; 4]>),
                Resolve,
                Fallback,
            }
            let action = {
                let cache = code.kwcall_cache.borrow();
                match &cache[pc - 1] {
                    KwCallCacheEntry::ExSimple {
                        param_binds_ptr,
                        npos: cnpos,
                        keyset,
                        slots,
                    } if param_binds_ptr.as_ptr() == pbptr
                        && *cnpos as usize == npos
                        && dict_keys_match(&kwargs_val, keyset) =>
                    {
                        Action::Hit(slots.clone())
                    }
                    KwCallCacheEntry::Fallback => Action::Fallback,
                    _ => Action::Resolve,
                }
            };

            match action {
                Action::Hit(slots) => {
                    return self.call_ex_fast_bind(
                        &f,
                        regs,
                        func,
                        npos,
                        &kwargs_val,
                        &slots,
                        num_locals,
                    );
                }
                Action::Resolve => {
                    // Read the dict's keys (in iteration order) as a kwnames vec to
                    // feed the shared resolver.  Bail to the slow path on any
                    // non-`str` key (CPython: "keywords must be strings").
                    let kwnames: Option<Vec<Value>> = kwargs_val
                        .dict_with(|d| {
                            let mut names: Vec<Value> = Vec::with_capacity(d.len());
                            for k in d.keys() {
                                match k {
                                    pyrust_core::PyKey::Str(s) => names.push(s.clone()),
                                    _ => return None,
                                }
                            }
                            Some(names)
                        })
                        .flatten();

                    if let Some(kwnames) = kwnames {
                        if let Some(slots) = Self::kwcall_resolve_simple(&f, npos, &kwnames) {
                            let keyset: smallvec::SmallVec<[Box<str>; 4]> = kwnames
                                .iter()
                                .map(|v| Box::<str>::from(v.as_str().unwrap_or("")))
                                .collect();
                            code.kwcall_cache.borrow_mut()[pc - 1] = KwCallCacheEntry::ExSimple {
                                param_binds_ptr: Rc::downgrade(&f.param_binds),
                                npos: npos as u8,
                                keyset,
                                slots: slots.clone(),
                            };
                            return self.call_ex_fast_bind(
                                &f,
                                regs,
                                func,
                                npos,
                                &kwargs_val,
                                &slots,
                                num_locals,
                            );
                        }
                        // Structurally not simple for this callee: pin Fallback so we
                        // don't rebuild kwnames every call (e.g. a `**kwargs` receiver,
                        // or a binding error the general binder will diagnose).
                        code.kwcall_cache.borrow_mut()[pc - 1] = KwCallCacheEntry::Fallback;
                    }
                    // Non-str key (or could not borrow) → slow path; do NOT pin
                    // Fallback, the general binder raises the right TypeError.
                }
                Action::Fallback => {}
            }
        }

        // Slow path: materialise positionals + `**d` entries into an
        // `ExpandedCallArg` buffer and dispatch through the general binder, which
        // owns CPython-parity binding + diagnostics for every callee / splat.
        let mut buf = std::mem::take(&mut self.call_arg_buf);
        buf.clear();
        for i in 0..npos as u32 {
            buf.push(ExpandedCallArg {
                name: None,
                value: vm_read(regs, func + 1 + i, num_locals)?,
            });
        }
        let extend_result = self.expand_kwargs_into(&func_val, &kwargs_val, &mut buf);
        let call_result = extend_result.and_then(|()| self.call_function_expanded(func_val, &buf));
        self.call_arg_buf = buf;
        call_result
    }

    /// Full body of `Insn::CallExArgs` — a positional-splat expansion call
    /// `f(<pos…>, *args[, **kw])` (the decorator/wrapper shape).  Leading
    /// positionals occupy `R[func+1 .. func+1+npos]`; `R[args_splat]` is the
    /// single `*args` iterable; `R[kwargs]` is the single `**kw` mapping, or
    /// [`crate::bytecode::NO_KWARGS`] when the call has no `**kw`.  Returns the
    /// call result.
    ///
    /// Fast path: a plain `Regular` `UserFunction` callee whose fixed-arity
    /// binding is simple, a `*args` splat that is a plain `tuple`/`list` (directly
    /// indexable — no user `__iter__`), and a `**kw` that is a plain `dict` (or
    /// absent).  A per-call-site shape cache ([`KwCallCacheEntry::ExArgs`]) keyed
    /// on `(param_binds identity, total positional count, dict key-set)` records
    /// the key→slot mapping; on a hit the leading positionals, splat elements, and
    /// dict values bind straight into their parameter slots via the #2382
    /// `call_user_function_kw_cached`, with no intermediate list/dict and no name
    /// scan.  A variadic (`*args`/`**kwargs`) callee, or any other non-simple
    /// binding shape, pins the site to `Fallback`; a non-tuple/list splat,
    /// non-dict `**kw`, non-`Regular` callee, or overridden `__defaults__` skips
    /// the fast path for that call without pinning.
    ///
    /// Slow path: materialise the leading positionals, the splat elements
    /// (honouring user `__iter__` / `__getitem__`, exactly as the old `ListExtend`
    /// lowering did), and the `**kw` entries into an `ExpandedCallArg` buffer — no
    /// intermediate list/dict and no hidden-global lookup or builtin dispatch —
    /// and dispatch through `call_function_expanded`, which owns every
    /// CPython-parity binding diagnostic.  The argument order is exactly the
    /// source order the compiler guarantees for this shape: leading positionals,
    /// then the splat elements, then the `**kw` entries.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn exec_call_ex_args(
        &mut self,
        regs: &RegSlice,
        code: &crate::bytecode::FnCode,
        pc: usize,
        func: crate::bytecode::Reg,
        npos: u8,
        nkw: u8,
        kwnames_idx: u16,
        args_splat: crate::bytecode::Reg,
        kwargs: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        use crate::bytecode::KwCallCacheEntry;
        let func_val = vm_read(regs, func, num_locals)?;
        let args_splat_val = vm_read(regs, args_splat, num_locals)?;
        let kwargs_val = if kwargs != crate::bytecode::NO_KWARGS {
            Some(vm_read(regs, kwargs, num_locals)?)
        } else {
            None
        };
        // Literal `kw=v` keyword arguments: values in `R[func+1+npos .. +nkw]`,
        // names in the `consts[kwnames_idx]` tuple.  Empty when `nkw == 0`.  The
        // shape check guarantees literal keywords never co-occur with a `**kw`
        // splat (a cross-source key collision needs DICT_MERGE), so these are the
        // only keywords when present.
        let lit_kw: smallvec::SmallVec<[(String, Value); 4]> = if nkw == 0 {
            smallvec::SmallVec::new()
        } else {
            let names = code
                .consts
                .get(kwnames_idx as usize)
                .and_then(|c| c.as_tuple());
            let mut v = smallvec::SmallVec::with_capacity(nkw as usize);
            for i in 0..nkw as u32 {
                let name = names
                    .and_then(|n| n.get(i as usize))
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_owned();
                v.push((name, vm_read(regs, func + 1 + npos as u32 + i, num_locals)?));
            }
            v
        };

        // Fast path requires a plain user function (Regular kind), a directly
        // indexable `*args` (plain tuple/list, whose iteration can't be user
        // overridden), and a plain `dict` (or absent) `**kw`.
        let user_fn = match func_val.kind() {
            ValueKind::UserFunction(f)
                if matches!(f.kind, pyrust_core::UserFunctionKind::Regular) =>
            {
                Some(Rc::clone(f))
            }
            _ => None,
        };
        let splat_len = args_splat_val
            .as_tuple()
            .map(|s| s.len())
            .or_else(|| args_splat_val.list_len());
        let kw_is_plain = kwargs_val
            .as_ref()
            .is_none_or(|v| matches!(v.kind(), ValueKind::Dict(_)));

        // #2395: bypass the kw-call cache when __defaults__ has been overridden.
        if let (Some(f), Some(slen), true) = (
            user_fn.filter(|f| f.defaults_override.borrow().is_none()),
            splat_len,
            kw_is_plain,
        ) {
            if nkw != 0 {
                // Literal keyword args present (and, per the shape check, no `**kw`
                // splat).  The fixed-arity slot cache keys on the `**kw` key-set, so
                // rather than fold static literal names into it, only the variadic
                // fast path applies here; a fixed-arity callee falls to the slow
                // path (which still uses the direct VM transport).
                if f.params.iter().any(|p| p.is_args || p.is_kwargs)
                    && let Some(v) = self.call_ex_args_variadic_bind(
                        &f,
                        regs,
                        func,
                        npos,
                        &lit_kw,
                        &args_splat_val,
                        None,
                        num_locals,
                    )?
                {
                    return Ok(v);
                }
            } else {
                let total_pos = npos as u32 + slen as u32;
                let pbptr = Rc::as_ptr(&f.param_binds);

                // A hit reuses `slots`; a `total_pos`/key-set/identity miss
                // re-resolves (unless pinned to `Fallback`).  With no `**kw` the
                // cached `keyset` must be empty.
                enum Action {
                    Hit(smallvec::SmallVec<[u32; 4]>),
                    /// Cached variadic callee; `pure_forward` selects the #2852
                    /// direct tuple/dict bind over the generic split forward.
                    VariadicHit {
                        pure_forward: bool,
                    },
                    Resolve,
                    Fallback,
                }
                let kw_keys_match = |keyset: &[Box<str>]| match &kwargs_val {
                    Some(d) => dict_keys_match(d, keyset),
                    None => keyset.is_empty(),
                };
                let action = {
                    let cache = code.kwcall_cache.borrow();
                    match &cache[pc - 1] {
                        KwCallCacheEntry::ExArgs {
                            param_binds_ptr,
                            total_pos: ctp,
                            keyset,
                            slots,
                        } if param_binds_ptr.as_ptr() == pbptr
                            && *ctp == total_pos
                            && kw_keys_match(keyset) =>
                        {
                            Action::Hit(slots.clone())
                        }
                        KwCallCacheEntry::ExArgsVariadic {
                            param_binds_ptr,
                            pure_forward,
                        } if param_binds_ptr.as_ptr() == pbptr => Action::VariadicHit {
                            pure_forward: *pure_forward,
                        },
                        KwCallCacheEntry::Fallback => Action::Fallback,
                        _ => Action::Resolve,
                    }
                };

                match action {
                    Action::Hit(slots) => {
                        return self.call_ex_args_fast_bind(
                            &f,
                            regs,
                            func,
                            npos,
                            &args_splat_val,
                            total_pos as usize,
                            kwargs_val.as_ref(),
                            &slots,
                            num_locals,
                        );
                    }
                    Action::VariadicHit { pure_forward } => {
                        // Pure `def inner(*A[, **K])` forward → build the tuple +
                        // dict directly (#2852); any other variadic shape → the
                        // generic split forward.  Both return `Ok(None)` on a
                        // non-`str` `**kw` key / unexpected-keyword, falling to the
                        // slow path for the CPython diagnostic.
                        let bound = if pure_forward {
                            self.call_ex_args_pure_forward_bind(
                                &f,
                                regs,
                                func,
                                npos,
                                &lit_kw,
                                &args_splat_val,
                                kwargs_val.as_ref(),
                                num_locals,
                            )?
                        } else {
                            self.call_ex_args_variadic_bind(
                                &f,
                                regs,
                                func,
                                npos,
                                &lit_kw,
                                &args_splat_val,
                                kwargs_val.as_ref(),
                                num_locals,
                            )?
                        };
                        if let Some(v) = bound {
                            return Ok(v);
                        }
                        // Fell through → slow path (non-str `**kw` key, or a pure
                        // `*A`-only callee handed unexpected keywords).
                    }
                    Action::Resolve => {
                        // Read the `**kw` keys (in iteration order) as a kwnames
                        // vec to feed the shared resolver (empty when no `**kw`).
                        // Bail to the slow path on any non-`str` key.
                        let kwnames: Option<Vec<Value>> = match &kwargs_val {
                            Some(d) => d
                                .dict_with(|dict| {
                                    let mut names: Vec<Value> = Vec::with_capacity(dict.len());
                                    for k in dict.keys() {
                                        match k {
                                            pyrust_core::PyKey::Str(s) => names.push(s.clone()),
                                            _ => return None,
                                        }
                                    }
                                    Some(names)
                                })
                                .flatten(),
                            None => Some(Vec::new()),
                        };

                        if let Some(kwnames) = kwnames {
                            if let Some(slots) =
                                Self::kwcall_resolve_simple(&f, total_pos as usize, &kwnames)
                            {
                                let keyset: smallvec::SmallVec<[Box<str>; 4]> = kwnames
                                    .iter()
                                    .map(|v| Box::<str>::from(v.as_str().unwrap_or("")))
                                    .collect();
                                code.kwcall_cache.borrow_mut()[pc - 1] = KwCallCacheEntry::ExArgs {
                                    param_binds_ptr: Rc::downgrade(&f.param_binds),
                                    total_pos,
                                    keyset,
                                    slots: slots.clone(),
                                };
                                return self.call_ex_args_fast_bind(
                                    &f,
                                    regs,
                                    func,
                                    npos,
                                    &args_splat_val,
                                    total_pos as usize,
                                    kwargs_val.as_ref(),
                                    &slots,
                                    num_locals,
                                );
                            }
                            // `kwcall_resolve_simple` rejected this callee.  If it
                            // is VARIADIC (`*args`/`**kwargs`), forward straight
                            // into `call_user_function_variadic_split` (skipping the
                            // buffer + second clone) and cache that; otherwise it is
                            // a fixed-arity binding error (missing/dup/posonly) whose
                            // diagnostics the general binder owns — pin Fallback.
                            if f.params.iter().any(|p| p.is_args || p.is_kwargs) {
                                // Detect the pure `def inner(*A[, **K])` forward
                                // shape once and cache it, so the shape check is
                                // paid only on this (cold) resolve, not per call.
                                let pure_forward = Self::is_pure_variadic_forward(&f);
                                code.kwcall_cache.borrow_mut()[pc - 1] =
                                    KwCallCacheEntry::ExArgsVariadic {
                                        param_binds_ptr: Rc::downgrade(&f.param_binds),
                                        pure_forward,
                                    };
                                let bound = if pure_forward {
                                    self.call_ex_args_pure_forward_bind(
                                        &f,
                                        regs,
                                        func,
                                        npos,
                                        &lit_kw,
                                        &args_splat_val,
                                        kwargs_val.as_ref(),
                                        num_locals,
                                    )?
                                } else {
                                    self.call_ex_args_variadic_bind(
                                        &f,
                                        regs,
                                        func,
                                        npos,
                                        &lit_kw,
                                        &args_splat_val,
                                        kwargs_val.as_ref(),
                                        num_locals,
                                    )?
                                };
                                if let Some(v) = bound {
                                    return Ok(v);
                                }
                                // Non-str `**kw` key / unexpected-keyword → slow path.
                            } else {
                                code.kwcall_cache.borrow_mut()[pc - 1] = KwCallCacheEntry::Fallback;
                            }
                        }
                        // Non-str key → slow path; do NOT pin Fallback, the general
                        // binder raises the right TypeError.
                    }
                    Action::Fallback => {}
                }
            }
        }

        // Slow path: materialise positionals + splat elements + `**kw` entries into
        // an `ExpandedCallArg` buffer and dispatch through the general binder.
        let mut buf = std::mem::take(&mut self.call_arg_buf);
        buf.clear();
        for i in 0..npos as u32 {
            buf.push(ExpandedCallArg {
                name: None,
                value: vm_read(regs, func + 1 + i, num_locals)?,
            });
        }
        let result = self
            .collect_call_splat(&func_val, &args_splat_val)
            .and_then(|items| {
                for value in items {
                    buf.push(ExpandedCallArg { name: None, value });
                }
                // Literal `kw=v` keyword args (empty unless `nkw > 0`; never co-occur
                // with `**kw` per the shape check).
                for (name, value) in &lit_kw {
                    buf.push(ExpandedCallArg {
                        name: Some(name.clone()),
                        value: value.clone(),
                    });
                }
                if let Some(kwargs_val) = &kwargs_val {
                    self.expand_kwargs_into(&func_val, kwargs_val, &mut buf)?;
                }
                self.call_function_expanded(func_val, &buf)
            });
        self.call_arg_buf = buf;
        result
    }
}
