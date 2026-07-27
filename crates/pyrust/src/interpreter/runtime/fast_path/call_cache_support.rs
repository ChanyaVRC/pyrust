/// Does a dict's ordered string-key shape match a cached `f(**kwargs)` site?
#[inline]
fn dict_keys_match(d: &Value, keyset: &[Box<str>]) -> bool {
    d.dict_with(|dict| {
        if dict.len() != keyset.len() {
            return false;
        }
        for (key, expected) in dict.keys().zip(keyset.iter()) {
            match key {
                pyrust_core::PyKey::Str(s) if s.as_str() == Some(expected.as_ref()) => {}
                _ => return false,
            }
        }
        true
    })
    .unwrap_or(false)
}

pub(super) enum MemoCallProbe {
    Bypass,
    Hit(Value),
    Miss(MemoKey),
}

impl Interpreter {
    /// Probe the adaptive result cache without executing the callee.
    ///
    /// An execution-free miss lets the opcode loop try its explicit frame
    /// trampoline before falling back to the native Rust call path.
    pub(super) fn probe_memoized_call(
        &mut self,
        registers: &RegSlice,
        function_register: crate::bytecode::Reg,
        argument_count: u8,
        number_of_locals: crate::bytecode::Reg,
        function: &Value,
    ) -> Result<MemoCallProbe> {
        let (function_id, positional_parameter_count) = match function.kind() {
            ValueKind::UserFunction(function) if function.is_memo_pure => {
                (function.id, function.memo_positional_parameter_count)
            }
            _ => return Ok(MemoCallProbe::Bypass),
        };
        // A raw CallMemo key contains only the supplied positional values.  If
        // a call omits a positional parameter, its result also depends on the
        // function's mutable `__defaults__` state.  Fall back to canonical call
        // binding unless every positional parameter is supplied explicitly.
        if u16::from(argument_count) != positional_parameter_count {
            return Ok(MemoCallProbe::Bypass);
        }
        if matches!(self.memo_stats.get(&function_id), Some((_, _, false))) {
            return Ok(MemoCallProbe::Bypass);
        }

        let mut key_arguments: smallvec::SmallVec<[i64; 3]> =
            smallvec::SmallVec::with_capacity(argument_count as usize);
        for offset in 0..crate::bytecode::Reg::from(argument_count) {
            let argument = vm_read(registers, function_register + 1 + offset, number_of_locals)?;
            let Some(integer) = argument.as_int() else {
                return Ok(MemoCallProbe::Bypass);
            };
            key_arguments.push(integer);
        }

        let key = (function_id, key_arguments);
        if let Some(cached) = self.memo_cache.get(&key) {
            let value = cached.clone();
            let stats = self.memo_stats.entry(function_id).or_insert((0, 0, true));
            stats.0 = stats.0.saturating_add(1);
            stats.1 = stats.1.saturating_add(1);
            return Ok(MemoCallProbe::Hit(value));
        }
        Ok(MemoCallProbe::Miss(key))
    }

    /// Claim a cache-miss key for one executing callee frame.
    ///
    /// `None` means an outer evaluation already owns the identical key. The
    /// nested call may still execute, but must not publish a second entry.
    #[inline]
    pub(super) fn claim_memoized_call(&mut self, key: MemoKey) -> Option<MemoKey> {
        self.memo_in_flight.insert(key.clone()).then_some(key)
    }

    /// Commit a successfully completed cache-miss owner.
    #[inline]
    pub(super) fn finish_memoized_call(&mut self, key: MemoKey, result: &Value) {
        let removed = self.memo_in_flight.remove(&key);
        debug_assert!(
            removed,
            "a completed memoized call must own an in-flight key"
        );

        let function_id = key.0;
        let stats = self.memo_stats.entry(function_id).or_insert((0, 0, true));
        stats.0 = stats.0.saturating_add(1);
        if stats.0 >= 128 && stats.1.saturating_mul(4) < stats.0 {
            stats.2 = false;
        }
        if stats.2
            && self.memo_cache.len() < (1usize << 16)
            && (matches!(result.kind(), ValueKind::Int(_) | ValueKind::Bool(_)) || result.is_none())
        {
            self.memo_cache.insert(key, result.clone());
        }
    }

    /// Cancel a cache-miss owner whose callee raised.
    #[inline]
    pub(super) fn cancel_memoized_call(&mut self, key: &MemoKey) {
        let removed = self.memo_in_flight.remove(key);
        debug_assert!(
            removed,
            "an aborted memoized call must own an in-flight key"
        );
    }

    /// Execute a probed miss through the native call path after the VM
    /// trampoline gate rejected it.
    pub(super) fn call_memoized_miss_native(
        &mut self,
        registers: &RegSlice,
        function_register: crate::bytecode::Reg,
        argument_count: u8,
        number_of_locals: crate::bytecode::Reg,
        function: &Value,
        key: MemoKey,
    ) -> Result<Value> {
        let mut buffer = std::mem::take(&mut self.call_arg_buf);
        buffer.clear();
        for offset in 0..crate::bytecode::Reg::from(argument_count) {
            buffer.push(ExpandedCallArg {
                name: None,
                value: vm_read(registers, function_register + 1 + offset, number_of_locals)?,
            });
        }
        let owned_key = self.claim_memoized_call(key);
        let call_result = self.call_function_expanded(function.clone(), &buffer);
        self.call_arg_buf = buffer;
        match call_result {
            Ok(result) => {
                if let Some(key) = owned_key {
                    self.finish_memoized_call(key, &result);
                }
                Ok(result)
            }
            Err(error) => {
                if let Some(key) = owned_key.as_ref() {
                    self.cancel_memoized_call(key);
                }
                Err(error)
            }
        }
    }

    /// Dispatch a warm positional built-in call without constructing an
    /// expanded-argument buffer.
    #[inline]
    pub(super) fn try_builtin_vectorcall(
        &mut self,
        code: &crate::bytecode::FnCode,
        call_site: usize,
        function: &Value,
        arguments: &[Value],
    ) -> Option<Result<Value>> {
        let ValueKind::BuiltinFunction(name) = function.kind() else {
            return None;
        };
        let dispatch = {
            let cache = code.call_builtin_cache.borrow();
            match cache.get(call_site)? {
                CallBuiltinCacheEntry::Cached {
                    name: cached_name,
                    fast: Some((dispatch, minimum, maximum)),
                    ..
                } if *cached_name == name
                    && (*minimum..=*maximum).contains(&(arguments.len() as u8)) =>
                {
                    *dispatch
                }
                _ => return None,
            }
        };
        if arguments.iter().any(Value::is_unset) {
            return None;
        }
        Some(dispatch(self, arguments))
    }

    /// Use the monomorphic per-call-site registry cache, populating it for a
    /// plain built-in on the first miss.
    fn call_with_builtin_site_cache(
        &mut self,
        code: &crate::bytecode::FnCode,
        call_site: usize,
        function: Value,
        arguments: &[ExpandedCallArg],
    ) -> Result<Value> {
        if let ValueKind::BuiltinFunction(name) = function.kind() {
            let cached = {
                let cache = code.call_builtin_cache.borrow();
                match cache.get(call_site) {
                    Some(CallBuiltinCacheEntry::Cached {
                        name: cached_name,
                        dispatch,
                        ..
                    }) if *cached_name == name => Some(*dispatch),
                    _ => None,
                }
            };
            if let Some(dispatch) = cached {
                return dispatch(self, arguments);
            }
            // Registry membership, not punctuation in the internal dispatch
            // key, determines cacheability.  Module functions such as
            // `math.sqrt` use a dotted key just like type descriptors, but
            // still have an immutable dispatcher and vectorcall entry.
            if let Some(registration) = crate::builtin_registry::lookup_registration(name) {
                let dispatch = registration.dispatch;
                code.call_builtin_cache.borrow_mut()[call_site] = CallBuiltinCacheEntry::Cached {
                    name,
                    dispatch,
                    fast: registration
                        .fast
                        .map(|fast| (fast, registration.min_arity, registration.max_arity)),
                };
                return dispatch(self, arguments);
            }
        }
        self.call_function_expanded(function, arguments)
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn call_positional_cached(
        &mut self,
        registers: &mut RegSlice,
        number_of_locals: crate::bytecode::Reg,
        function_register: crate::bytecode::Reg,
        argument_count: u8,
        function: Value,
        code: &crate::bytecode::FnCode,
        call_site: usize,
        current_line: u32,
    ) -> Result<Value> {
        let argument_base = (function_register + 1) as usize;
        let argument_end = argument_base + argument_count as usize;
        let lend_by_move = should_lend_builtin_call_arguments(
            &function,
            &(**registers)[argument_base..argument_end],
        );

        // Validate before moving anything so an error cannot leave temporary
        // registers unset.
        if lend_by_move {
            for offset in 0..crate::bytecode::Reg::from(argument_count) {
                let register = function_register + 1 + offset;
                if registers[register as usize].is_unset() {
                    vm_read(registers, register, number_of_locals)?;
                }
            }
        }

        let mut buffer = std::mem::take(&mut self.call_arg_buf);
        buffer.clear();
        let fill_result = if lend_by_move {
            for offset in 0..crate::bytecode::Reg::from(argument_count) {
                let register = function_register + 1 + offset;
                let value = std::mem::replace(
                    &mut registers[register as usize],
                    pyrust_core::Value::unset(),
                );
                buffer.push(ExpandedCallArg { name: None, value });
            }
            Ok(())
        } else {
            (|| {
                for offset in 0..crate::bytecode::Reg::from(argument_count) {
                    buffer.push(ExpandedCallArg {
                        name: None,
                        value: vm_read(
                            registers,
                            function_register + 1 + offset,
                            number_of_locals,
                        )?,
                    });
                }
                Ok(())
            })()
        };
        if let Err(error) = fill_result {
            self.call_arg_buf = buffer;
            return Err(error);
        }

        Self::publish_frame_line_for_builtin(&function, current_line);
        let result = self.call_with_builtin_site_cache(code, call_site, function, &buffer);

        if lend_by_move {
            for (offset, argument) in buffer.drain(..).enumerate() {
                registers[(function_register + 1 + offset as crate::bytecode::Reg) as usize] =
                    argument.value;
            }
        }
        self.call_arg_buf = buffer;
        result
    }
}

#[inline]
fn should_lend_builtin_call_arguments(function: &Value, arguments: &[Value]) -> bool {
    matches!(function.kind(), ValueKind::BuiltinFunction(_))
        && arguments.iter().any(Value::is_heap_tuple)
}
