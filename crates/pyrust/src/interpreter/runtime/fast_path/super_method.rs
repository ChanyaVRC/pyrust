// Fused zero-argument `super().name(...)` call (#3424).
//
// The compiler lowers `super().name(args)` to
//
//     LoadGlobal       callee <- `super`
//     LoadSuperMethod  func, recv <- callee, `name`
//     ...argument evaluation...
//     CallSuperMethod  dst <- func, recv, args
//
// The generic sequence it replaces allocates a `super` object, linearizes the
// receiver's MRO to find `name`, allocates a bound method, and calls it. The
// resolved form does none of that: the method comes from a per-site cache
// keyed by the receiver class and the defining class, and is entered with the
// receiver as its first argument.
//
// Whatever the resolved form cannot prove falls back to the generic sequence,
// executed by the same code that runs an unfused `super()` call and a
// `CallMethod`, so every error, descriptor, and rebinding keeps its behaviour.

impl Interpreter {
    /// Resolved form of `Insn::LoadSuperMethod`: the `(function, receiver)`
    /// pair to call, or `None` to take the generic form.
    ///
    /// `callee` is the value `LoadGlobal super` produced. The frame's
    /// `__class__` cell and first argument are read through
    /// `resolve_zero_arg_super`, the same service the `super` builtin uses, so
    /// both forms always see the same class and receiver — including inside
    /// comprehensions, closures, and generators.
    ///
    /// Declines (returns `None`, without raising) when:
    /// - `callee` is not the canonical builtin `super`;
    /// - a trace or profile hook is installed (the builtin call is an
    ///   observable `c_call` event);
    /// - zero-argument `super()` itself would fail, or the first argument is
    ///   not a user instance (a classmethod's `cls`);
    /// - the lookup does not resolve to a plain Python function.
    #[inline]
    pub(super) fn try_load_super_method(
        &self,
        callee: &Value,
        name_idx: u16,
        code: &crate::bytecode::FnCode,
        call_site_pc: usize,
    ) -> Option<(Value, Value)> {
        use crate::bytecode::SuperMethodProbe;

        // A builtin callable dispatches purely by the name its value kind
        // carries, so this is the identity of the real `super`: a module-level
        // rebinding, a `builtins.super` patch, or a shadowing local all load
        // some other value.
        if !matches!(callee.kind(), ValueKind::BuiltinFunction("super"))
            || self.vm_hooks_flags & (VM_HOOK_TRACE | VM_HOOK_PROFILE) != 0
        {
            return None;
        }
        // One cache borrow covers the whole hit path. Nothing below runs
        // Python code or touches this cache until the borrow is released.
        let cache = code.super_method_cache.borrow();
        let site = cache.get(call_site_pc);
        if site.is_some_and(crate::bytecode::SuperMethodSite::is_generic) {
            return None;
        }

        let (class_value, receiver) = resolve_zero_arg_super(self).ok()?;
        let ValueKind::PyClass(start_class) = class_value.kind() else {
            return None;
        };
        let instance = receiver.as_py_instance_rc()?;
        let (self_class, class_version) = {
            let instance = instance.borrow();
            let version = instance.class.borrow().mutation_version.get();
            (Rc::clone(&instance.class), version)
        };

        let probe = match site {
            Some(site) => site.probe(&self_class, start_class, class_version),
            None => SuperMethodProbe::Miss,
        };
        drop(cache);
        match probe {
            SuperMethodProbe::Method(function) => return Some((function, receiver)),
            SuperMethodProbe::NotPlainMethod => return None,
            SuperMethodProbe::Miss => {}
        }

        // Resolve once along the MRO, then remember the answer — also when it
        // is "not a plain method", so such a site stops paying for the walk.
        let name = code.names.get(name_idx as usize)?.as_str();
        let target = super_plain_method(start_class, instance, name);
        if let Some(site) = code
            .super_method_cache
            .borrow_mut()
            .entry_or_default(call_site_pc)
        {
            site.record(&self_class, start_class, class_version, target.as_ref());
        }
        target.map(|function| (function, receiver))
    }

    /// Generic form of `Insn::LoadSuperMethod`: call `R[callee]` with no
    /// arguments, exactly as the `Call(callee, 0)` this instruction replaces.
    #[inline(never)]
    pub(super) fn call_super_callee(
        &mut self,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
        callee: crate::bytecode::Reg,
        code: &crate::bytecode::FnCode,
        call_site_pc: usize,
    ) -> Result<Value> {
        let function = vm_read(regs, callee, num_locals)?;
        // Same dispatch ladder as the plain `Call` arm: a cached vectorcall
        // entry runs directly, and a profiled builtin call stays observable by
        // bypassing the call-site cache.
        let profile_disabled = self.vm_hooks_flags & VM_HOOK_PROFILE == 0;
        let probe = match Self::probe_builtin_vectorcall(code, call_site_pc, &function, &[]) {
            BuiltinCallProbe::Vector(dispatch) if profile_disabled => {
                return dispatch(self, &[]);
            }
            probe if profile_disabled => probe,
            _ => BuiltinCallProbe::Uncacheable,
        };
        self.call_positional_cached(
            regs,
            num_locals,
            callee,
            0,
            function,
            code,
            call_site_pc,
            PositionalCallCacheProbe::Probed(probe),
        )
    }

    /// The function a resolved `LoadSuperMethod` left in `R[func]` (its
    /// receiver is `R[recv]`), or `None` for the generic form, whose `R[recv]`
    /// is `None` and never an instance.
    #[inline(always)]
    pub(super) fn resolved_super_method(
        regs: &RegSlice,
        func: crate::bytecode::Reg,
        recv: crate::bytecode::Reg,
    ) -> Option<Rc<UserFunction>> {
        if !matches!(regs[recv as usize].kind(), ValueKind::PyInstance(_)) {
            return None;
        }
        match regs[func as usize].kind() {
            ValueKind::UserFunction(function) => Some(Rc::clone(function)),
            _ => None,
        }
    }

    /// Resolved form of `Insn::CallSuperMethod` for a callee the VM could not
    /// trampoline (a `try` around the call site, a `*args` method, a
    /// generator, an active hook, ...): `function(receiver, *args)` through the
    /// canonical binder, the call a bound method makes.
    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    pub(super) fn call_resolved_super_method(
        &mut self,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        function: &Rc<UserFunction>,
        func: crate::bytecode::Reg,
        recv: crate::bytecode::Reg,
        args_base: crate::bytecode::Reg,
        nargs: u8,
        code: &crate::bytecode::FnCode,
        call_site_pc: usize,
    ) -> Result<Value> {
        let args = Self::read_call_method_args(regs, args_base, nargs, num_locals)?;
        let receiver = regs[recv as usize].clone();
        // Publish the call-site line for the callee's frame introspection, as
        // the generic method call does.
        let cur_line = code.line_at(call_site_pc);
        if cur_line != 0 {
            self.cold
                .call_site_lines
                .push((regs.non_null_ptr().as_ptr(), cur_line));
        }
        let result = if Self::is_varargs_positional_callee(function) {
            self.call_varargs_positional(function, Some(&receiver), &args)
        } else {
            let unbound = regs[func as usize].clone();
            let mut buf = std::mem::take(&mut self.call_arg_buf);
            buf.clear();
            buf.extend(
                args.into_iter()
                    .map(|value| ExpandedCallArg { name: None, value }),
            );
            let result = invoke_class_method(self, unbound, receiver, &buf);
            buf.clear();
            self.call_arg_buf = buf;
            result
        };
        if cur_line != 0 {
            self.cold.call_site_lines.pop();
        }
        result
    }
}
