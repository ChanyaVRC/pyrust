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

pub(super) type BuiltinVectorcallDispatch = crate::builtin_registry::BuiltinFastDispatchFn;

#[derive(Clone, Copy)]
pub(super) enum BuiltinCallCacheMiss {
    Registry,
    Class,
}

/// Execution-free result of the single call-site cache probe.
#[derive(Clone, Copy)]
pub(super) enum BuiltinCallProbe {
    Uncacheable,
    Vector(BuiltinVectorcallDispatch),
    Expanded(crate::builtin_registry::BuiltinDispatchFn),
    ClassAfterPrimitiveMiss,
    EligibleMiss(BuiltinCallCacheMiss),
}

/// Plain `Call` carries an already-probed token. `CallMemo` bypasses have not
/// visited this independent cache and request exactly one internal probe.
#[derive(Clone, Copy)]
pub(super) enum PositionalCallCacheProbe {
    Probed(BuiltinCallProbe),
    Unprobed,
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

    /// Probe a positional built-in call without executing the callee.
    ///
    /// Value kinds outside the registry/class cache domain return before the
    /// RefCell borrow. Eligible values read the cache exactly once and return a
    /// copied token that is consumed only after every cache/register borrow has
    /// ended.
    #[inline]
    pub(super) fn probe_builtin_vectorcall(
        code: &crate::bytecode::FnCode,
        call_site: usize,
        function: &Value,
        arguments: &[Value],
    ) -> BuiltinCallProbe {
        Self::probe_builtin_call_cache(code, call_site, function, Some(arguments))
    }

    #[inline]
    fn cached_builtin_call_probe(
        dispatch: crate::builtin_registry::BuiltinDispatchFn,
        fast: Option<(BuiltinVectorcallDispatch, u8, u8)>,
        arguments: Option<&[Value]>,
    ) -> BuiltinCallProbe {
        if let (Some((fast, minimum, maximum)), Some(arguments)) = (fast, arguments)
            && (minimum..=maximum).contains(&(arguments.len() as u8))
            && !arguments.iter().any(Value::is_unset)
        {
            BuiltinCallProbe::Vector(fast)
        } else {
            BuiltinCallProbe::Expanded(dispatch)
        }
    }

    #[inline]
    fn probe_builtin_call_cache(
        code: &crate::bytecode::FnCode,
        call_site: usize,
        function: &Value,
        vector_arguments: Option<&[Value]>,
    ) -> BuiltinCallProbe {
        match function.kind() {
            ValueKind::BuiltinFunction(name) => {
                let cache = code.call_builtin_cache.borrow();
                match cache.get(call_site) {
                    Some(CallBuiltinCacheEntry::Cached {
                        key: super::formatting::CallBuiltinCacheKey::RegistryName(cached_name),
                        dispatch,
                        fast,
                    }) if *cached_name == name => {
                        Self::cached_builtin_call_probe(*dispatch, *fast, vector_arguments)
                    }
                    _ => BuiltinCallProbe::EligibleMiss(BuiltinCallCacheMiss::Registry),
                }
            }
            ValueKind::PyClass(class) => {
                let cache = code.call_builtin_cache.borrow();
                match cache.get(call_site) {
                    Some(CallBuiltinCacheEntry::Cached {
                        key: super::formatting::CallBuiltinCacheKey::PrimitiveClass(cached_class),
                        dispatch,
                        fast,
                    }) if cached_class.as_ptr() == std::rc::Rc::as_ptr(class) => {
                        Self::cached_builtin_call_probe(*dispatch, *fast, vector_arguments)
                    }
                    Some(CallBuiltinCacheEntry::ClassAfterPrimitiveMiss(cached_class))
                        if cached_class.as_ptr() == std::rc::Rc::as_ptr(class) =>
                    {
                        BuiltinCallProbe::ClassAfterPrimitiveMiss
                    }
                    _ => BuiltinCallProbe::EligibleMiss(BuiltinCallCacheMiss::Class),
                }
            }
            _ => BuiltinCallProbe::Uncacheable,
        }
    }

    /// Consume one execution-free probe token, populating a cacheable miss
    /// without reading the call-site cache a second time.
    fn call_with_builtin_site_cache(
        &mut self,
        code: &crate::bytecode::FnCode,
        call_site: usize,
        function: Value,
        arguments: &[ExpandedCallArg],
        probe: BuiltinCallProbe,
    ) -> Result<Value> {
        match probe {
            BuiltinCallProbe::Uncacheable => self.call_function_expanded(function, arguments),
            BuiltinCallProbe::Vector(_) => {
                unreachable!("vector tokens are consumed by the opcode loop")
            }
            BuiltinCallProbe::Expanded(dispatch) => dispatch(self, arguments),
            BuiltinCallProbe::ClassAfterPrimitiveMiss => {
                let ValueKind::PyClass(class) = function.kind() else {
                    unreachable!("a negative class token must retain its class callee")
                };
                self.call_class_after_primitive_miss(std::rc::Rc::clone(class), arguments)
            }
            BuiltinCallProbe::EligibleMiss(BuiltinCallCacheMiss::Registry) => {
                let ValueKind::BuiltinFunction(name) = function.kind() else {
                    unreachable!("a registry miss token must retain its registry callee")
                };
                let Some(registration) = crate::builtin_registry::lookup_registration(name) else {
                    return self.call_function_expanded(function, arguments);
                };
                let fast = registration
                    .fast
                    .map(|fast| (fast, registration.min_arity, registration.max_arity));
                code.call_builtin_cache.borrow_mut()[call_site] = CallBuiltinCacheEntry::Cached {
                    key: super::formatting::CallBuiltinCacheKey::RegistryName(name),
                    dispatch: registration.dispatch,
                    fast,
                };
                (registration.dispatch)(self, arguments)
            }
            BuiltinCallProbe::EligibleMiss(BuiltinCallCacheMiss::Class) => {
                let ValueKind::PyClass(class) = function.kind() else {
                    unreachable!("a class miss token must retain its class callee")
                };
                let class = std::rc::Rc::clone(class);

                // This is the sole primitive-map probe on a cold class miss.
                let Some(dispatch) = super::primitive_class_dispatch(&class) else {
                    code.call_builtin_cache.borrow_mut()[call_site] =
                        CallBuiltinCacheEntry::ClassAfterPrimitiveMiss(std::rc::Rc::downgrade(
                            &class,
                        ));
                    return self.call_class_after_primitive_miss(class, arguments);
                };

                // Positive dispatch is safe during an internal class borrow
                // conflict, but metadata observed under that conflict must not
                // be retained.
                let builtin_type_tag = match super::try_builtin_type_class_tag(&class) {
                    Ok(tag) => tag,
                    Err(_) => return dispatch(self, arguments),
                };
                let fast = builtin_type_tag.and_then(|tag| {
                    let name = super::BuiltinTypeClass::from_tag(tag).class_name();
                    crate::builtin_registry::lookup_registration(name).and_then(|registration| {
                        registration
                            .fast
                            .map(|fast| (fast, registration.min_arity, registration.max_arity))
                    })
                });
                code.call_builtin_cache.borrow_mut()[call_site] = CallBuiltinCacheEntry::Cached {
                    key: super::formatting::CallBuiltinCacheKey::PrimitiveClass(
                        std::rc::Rc::downgrade(&class),
                    ),
                    dispatch,
                    fast,
                };
                dispatch(self, arguments)
            }
        }
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
        probe: PositionalCallCacheProbe,
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

        let probe = match probe {
            PositionalCallCacheProbe::Probed(probe) => probe,
            PositionalCallCacheProbe::Unprobed => {
                Self::probe_builtin_call_cache(code, call_site, &function, None)
            }
        };
        Self::publish_frame_line_for_builtin(&function, current_line);
        let result = self.call_with_builtin_site_cache(code, call_site, function, &buffer, probe);

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
