/// Heap-allocated state for a built-in iterable wrapped by `iter()`.
/// Stored type-erased inside `Value::generator()` via `Box<dyn Any>`,
/// the same slot used for GeneratorFrame.  resume_generator() checks
/// which concrete type it has by downcasting.
pub(crate) struct NativeIterFrame {
    pub(crate) items: Vec<Value>,
    pub(crate) pos: usize,
}

/// Heap-allocated execution state for a suspended generator.
/// Stored type-erased inside `Value::generator()` via `Box<dyn Any>`.
pub(crate) struct GeneratorFrame {
    pub(crate) code: Rc<crate::bytecode::FnCode>,
    pub(crate) regs: Vec<Value>,
    pub(crate) iters: Vec<Option<IterState>>,
    pub(crate) exc_handlers: Vec<usize>,
    /// Program counter for the NEXT instruction to execute on resumption.
    pub(crate) pc: usize,
    pub(crate) done: bool,
    /// The environment (closure captures) active when the generator was created.
    pub(crate) saved_env: EnvRef,
}

// Thread-local used to pass generator suspension state back from the VM loop
// to the resume_generator() caller without an extra return value or RefCell on
// the hot path.  Set immediately before `return Err(GeneratorYield(...))`.
thread_local! {
    static GEN_SAVE: std::cell::RefCell<Option<(Vec<Option<IterState>>, Vec<usize>, usize)>>
        = const { std::cell::RefCell::new(None) };
}

#[derive(Clone)]
enum IterState {
    Materialized(Vec<Value>, usize),
    Range { cur: i64, stop: i64, step: i64 },
    /// Lazy: reads directly from the source register on each ForIter call.
    /// Avoids the O(n) upfront clone that Materialized would require for List/Tuple.
    /// Behaves like CPython's list_iterator: checks pos < len each tick; no
    /// mutation detection (appending extends iteration, removing shortens it).
    Indexed { reg: crate::bytecode::Reg, pos: usize },
    /// Lazy enumerate: yields (counter, item) pairs from pre-materialized items.
    /// The source is materialised once at GetIter time, but individual tuples are
    /// built on demand instead of all at once.
    Enumerate { items: Vec<Value>, pos: usize, counter: i64 },
    /// Lazy zip: yields row-tuples from pre-materialized parallel sources.
    Zip { sources: Vec<Vec<Value>>, pos: usize, len: usize },
    /// Lazy reversed: walks a materialized Vec from end to start without mutating it.
    ReversedItems { items: Vec<Value>, pos: usize },
    /// User-defined iterator: holds the iterator object (result of __iter__).
    /// Each ForIter call invokes __next__() on it and stops on StopIteration.
    UserDefined(Value),
}

fn int_int_fast(a: i64, b: i64, op: BinaryOp) -> Option<Value> {
    match op {
        BinaryOp::Add    => a.checked_add(b).map(Value::int),
        BinaryOp::Sub    => a.checked_sub(b).map(Value::int),
        BinaryOp::Mul    => a.checked_mul(b).map(Value::int),
        BinaryOp::BitAnd => Some(Value::int(a & b)),
        BinaryOp::BitOr  => Some(Value::int(a | b)),
        BinaryOp::BitXor => Some(Value::int(a ^ b)),
        BinaryOp::LShift => if b < 0 { None } else { Some(Value::int(a << (b & 63))) },
        BinaryOp::RShift => if b < 0 { None } else { Some(Value::int(a >> (b & 63))) },
        BinaryOp::Eq  => Some(Value::bool_(a == b)),
        BinaryOp::Ne  => Some(Value::bool_(a != b)),
        BinaryOp::Lt  => Some(Value::bool_(a < b)),
        BinaryOp::Le  => Some(Value::bool_(a <= b)),
        BinaryOp::Gt  => Some(Value::bool_(a > b)),
        BinaryOp::Ge  => Some(Value::bool_(a >= b)),
        _ => None,
    }
}

fn int_cmp(a: i64, b: i64, op: BinaryOp) -> Option<bool> {
    match op {
        BinaryOp::Eq => Some(a == b),
        BinaryOp::Ne => Some(a != b),
        BinaryOp::Lt => Some(a < b),
        BinaryOp::Le => Some(a <= b),
        BinaryOp::Gt => Some(a > b),
        BinaryOp::Ge => Some(a >= b),
        _ => None,
    }
}

fn expect_list_mut<'a>(
    regs: &'a mut [Value],
    reg: u32,
    op: &str,
) -> Result<&'a mut Vec<Value>> {
    regs[reg as usize]
        .as_list_mut()
        .ok_or_else(|| PyError::Runtime(format!("{op} on non-list")))
}

fn expect_dict_mut<'a>(
    regs: &'a mut [Value],
    reg: u32,
    op: &str,
) -> Result<&'a mut indexmap::IndexMap<PyKey, Value>> {
    regs[reg as usize]
        .as_dict_mut()
        .ok_or_else(|| PyError::Runtime(format!("{op} on non-dict")))
}

impl Interpreter {
    /// Execute compiled bytecode for a user function.
    ///
    /// `regs` must be pre-sized to `code.num_regs` with parameter slots already filled.
    fn run_bytecode(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &mut [Value],
    ) -> Result<Value> {
        self.run_bytecode_inner(
            code,
            regs,
            vec![None; code.num_iters as usize],
            Vec::new(),
            0,
            None,
        )
    }

    /// Like `run_bytecode` but also passes the current function's id so that
    /// `TailCall` instructions can perform self-call detection.
    fn run_bytecode_for_fn(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &mut [Value],
        fn_id: u64,
    ) -> Result<Value> {
        self.run_bytecode_inner(
            code,
            regs,
            vec![None; code.num_iters as usize],
            Vec::new(),
            0,
            Some(fn_id),
        )
    }

    /// Resume (or initialise) a generator by executing from `frame.pc` until
    /// the next yield or completion.  Returns:
    /// - `Ok(val)`  — generator returned (StopIteration); frame.done = true
    /// - `Err(GeneratorYield(val))` — generator yielded; frame updated in-place
    /// - `Err(other)` — propagating exception
    pub(crate) fn resume_generator(&mut self, frame: &mut GeneratorFrame) -> Result<Value> {
        if frame.done {
            return Err(PyError::Named(
                "StopIteration".to_string(),
                String::new(),
            ));
        }

        // Swap the saved env in.
        let previous_env = std::mem::replace(&mut self.env, Rc::clone(&frame.saved_env));

        let result = self.run_bytecode_inner(
            &frame.code.clone(),
            &mut frame.regs,
            std::mem::take(&mut frame.iters),
            std::mem::take(&mut frame.exc_handlers),
            frame.pc,
            None,
        );

        // Restore env.
        self.env = previous_env;

        match result {
            Err(PyError::GeneratorYield(val)) => {
                // Retrieve the saved state from the thread-local.
                let saved = GEN_SAVE.with(|cell| cell.borrow_mut().take());
                if let Some((saved_iters, saved_handlers, saved_pc)) = saved {
                    frame.iters = saved_iters;
                    frame.exc_handlers = saved_handlers;
                    frame.pc = saved_pc;
                } else {
                    unreachable!("GEN_SAVE must be set before every GeneratorYield");
                }
                Err(PyError::GeneratorYield(val))
            }
            Ok(_return_val) => {
                // Generator returned normally (fell off end or hit explicit `return`).
                // Signal exhaustion as StopIteration so ForIter and call_next handle it uniformly.
                frame.done = true;
                Err(PyError::Named("StopIteration".to_string(), String::new()))
            }
            Err(e) => {
                // Propagating exception or other error.
                frame.done = true;
                Err(e)
            }
        }
    }

    fn run_bytecode_inner(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &mut [Value],
        iters_init: Vec<Option<IterState>>,
        exc_handlers_init: Vec<usize>,
        start_pc: usize,
        current_fn_id: Option<u64>,
    ) -> Result<Value> {
        use crate::bytecode::Insn;
        use std::collections::HashMap;
        let num_locals = code.num_locals;

        let mut iters: Vec<Option<IterState>> = iters_init;
        let mut exc_handlers: Vec<usize> = exc_handlers_init;
        let mut pc: usize = start_pc;
        // Counts self-tail-call iterations so that infinite tail recursion
        // eventually raises RecursionError instead of looping forever.
        let mut tco_iters: usize = 0;

        'vm: loop {
        // Dispatch errors through the active exception handler stack.
        // Defined inside the loop so `continue 'vm` resolves to this loop.
        macro_rules! vm_try {
            ($expr:expr) => {
                match $expr {
                    Ok(v) => v,
                    Err(e) => {
                        if let Some(h) = exc_handlers.pop() {
                            let exc_val = match e {
                                PyError::Raised(v) => v,
                                PyError::Runtime(msg) => {
                                    match self.instantiate_named_exception("RuntimeError", msg) {
                                        Ok(v) => v,
                                        Err(e2) => return Err(e2),
                                    }
                                }
                                PyError::Named(cls, msg) => {
                                    match self.instantiate_named_exception(&cls, msg) {
                                        Ok(v) => v,
                                        Err(e2) => return Err(e2),
                                    }
                                }
                                other => return Err(other),
                            };
                            self.active_exception = Some(exc_val);
                            pc = h;
                            continue 'vm;
                        } else {
                            return Err(e);
                        }
                    }
                }
            };
        }
        macro_rules! pool_get {
            ($pool:expr, $idx:expr, $tag:literal) => {
                match ($pool).get($idx as usize) {
                    Some(v) => v,
                    None => {
                        vm_try!(Err(PyError::Runtime(format!(
                            "bytecode error: {} index {} out of range (pool size {})",
                            $tag, $idx, ($pool).len()
                        ))));
                        unreachable!()
                    }
                }
            };
        }
            let Some(insn) = code.insns.get(pc) else {
                if pc == code.insns.len() {
                    return Ok(Value::none());
                }
                return Err(PyError::Runtime(format!(
                    "internal error: PC {} out of bounds (insns len {})",
                    pc,
                    code.insns.len()
                )));
            };
            pc += 1;

            macro_rules! jump_pc {
                ($offset:expr) => {{
                    let new_pc = pc as i64 + $offset as i64;
                    if new_pc < 0 || new_pc as usize > code.insns.len() {
                        return Err(PyError::Runtime(format!(
                            "internal error: jump to invalid PC {} (insns len {})",
                            new_pc,
                            code.insns.len()
                        )));
                    }
                    new_pc as usize
                }};
            }

            match insn {
                // ── Loads ────────────────────────────────────────────────
                Insn::LoadConst(dst, idx) => {
                    let cv = pool_get!(code.consts, *idx, "const");
                    if let ValueKind::Int(n) = cv.kind() {
                        regs[*dst as usize] = Value::int(n);
                    } else {
                        regs[*dst as usize] = cv.clone();
                    }
                }
                Insn::LoadGlobal(dst, name_idx) => {
                    let name = pool_get!(code.names, *name_idx, "name");
                    let val = if let Some(v) = vm_try!(self.lookup_name(name)) {
                        v
                    } else {
                        vm_try!(resolve_builtin(name).ok_or_else(|| {
                            PyError::Runtime(format!("name '{}' is not defined", name))
                        }))
                    };
                    regs[*dst as usize] = val;
                }
                Insn::StoreGlobal(name_idx, src) => {
                    let name = pool_get!(code.names, *name_idx, "name").clone();
                    let val = vm_try!(vm_read(regs, *src, num_locals));
                    self.assign_name(name, val);
                }
                Insn::LoadNone(dst) => {
                    regs[*dst as usize] = Value::none();
                }
                Insn::Move(dst, src) | Insn::CopyReg(dst, src) => {
                    if let Some(v) = regs[*src as usize].as_some()
                        && let ValueKind::Int(n) = v.kind() {
                            regs[*dst as usize] = Value::int(n);
                            continue;
                        }
                    let v = vm_try!(vm_read(regs, *src, num_locals));
                    regs[*dst as usize] = v;
                }

                // ── Arithmetic / Logic ───────────────────────────────────
                Insn::BinOp(dst, lhs, op, rhs) => {
                    let lv = &regs[*lhs as usize];
                    let rv = &regs[*rhs as usize];
                    if let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), rv.kind())
                        && let Some(result) = int_int_fast(a, b, *op) {
                            regs[*dst as usize] = result;
                            continue;
                        }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
                    regs[*dst as usize] = vm_try!(self.eval_binary(l, *op, r));
                }
                Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                    let lv = &regs[*lhs as usize];
                    let rv = &regs[*rhs as usize];
                    if let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), rv.kind())
                        && let Some(result) = int_int_fast(a, b, *op) {
                            regs[*dst as usize] = result;
                            continue;
                        }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
                    let result = if let Some(v) = vm_try!(self.try_inplace_op(l.clone(), *op, r.clone())) {
                        v
                    } else {
                        vm_try!(self.eval_binary(l, *op, r))
                    };
                    regs[*dst as usize] = result;
                }
                Insn::BinOpConst(dst, lhs, op, const_idx) => {
                    let cv = pool_get!(code.consts, *const_idx, "const");
                    if let Some(lv) = regs[*lhs as usize].as_some()
                        && let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), cv.kind())
                            && let Some(result) = int_int_fast(a, b, *op) {
                                regs[*dst as usize] = result;
                                continue;
                            }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = cv.clone();
                    let result = if let Some(v) = vm_try!(self.try_inplace_op(l.clone(), *op, r.clone())) {
                        v
                    } else {
                        vm_try!(self.eval_binary(l, *op, r))
                    };
                    regs[*dst as usize] = result;
                }
                Insn::UnaryOp(dst, op, src) => {
                    let val = vm_try!(vm_read(regs, *src, num_locals));
                    let result = if *op == UnaryOp::Not {
                        // Dispatch __bool__ for instances before falling back to truthy().
                        Value::bool_(!vm_try!(self.truthy_value(&val)))
                    } else {
                        // Try dunder methods on PyInstance before the built-in path.
                        let dunder = match op {
                            UnaryOp::Neg => Some("__neg__"),
                            UnaryOp::Pos => Some("__pos__"),
                            UnaryOp::BitNot => Some("__invert__"),
                            UnaryOp::Not => None,
                        };
                        if let Some(dunder_name) = dunder {
                            if let Some(r) = self.try_dunder_unary(&val, dunder_name) {
                                vm_try!(r)
                            } else {
                                vm_try!(vm_eval_unary(*op, val))
                            }
                        } else {
                            vm_try!(vm_eval_unary(*op, val))
                        }
                    };
                    regs[*dst as usize] = result;
                }

                // ── Attribute / Index ────────────────────────────────────
                Insn::GetAttr(dst, obj, name_idx) => {
                    let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    let result = vm_try!(self.get_attr(obj_val, name));
                    regs[*dst as usize] = result;
                }
                Insn::SetAttr(obj, name_idx, val) => {
                    let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                    let val_val = vm_try!(vm_read(regs, *val, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    vm_try!(self.assign_attr(obj_val, name, val_val));
                }
                Insn::DeleteAttr(obj, name_idx) => {
                    let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    vm_try!(self.delete_attr(obj_val, name));
                }
                Insn::GetItem(dst, obj, idx) => {
                    // Fast path: List/Tuple indexed by Int — borrow idx, avoid clone.
                    let fast_int_idx = if let Some(iv) = regs[*idx as usize].as_some() {
                        if let ValueKind::Int(raw_i) = iv.kind() { Some(raw_i) } else { None }
                    } else { None };

                    if let Some(raw_i) = fast_int_idx {
                        let mut handled = false;
                        if let Some(ov) = regs[*obj as usize].as_some() {
                            match ov.kind() {
                                ValueKind::List(items) => {
                                    let len = items.len() as i64;
                                    let j = if raw_i < 0 { raw_i + len } else { raw_i };
                                    if j >= 0 && (j as usize) < items.len() {
                                        regs[*dst as usize] = items[j as usize].clone();
                                    } else {
                                        vm_try!(Err(PyError::Named("IndexError".into(), "list index out of range".into())));
                                    }
                                    handled = true;
                                }
                                ValueKind::Tuple(items) => {
                                    let len = items.len() as i64;
                                    let j = if raw_i < 0 { raw_i + len } else { raw_i };
                                    if j >= 0 && (j as usize) < items.len() {
                                        regs[*dst as usize] = items[j as usize].clone();
                                    } else {
                                        vm_try!(Err(PyError::Named("IndexError".into(), "tuple index out of range".into())));
                                    }
                                    handled = true;
                                }
                                _ => {}
                            }
                        }
                        if handled { continue; }
                    }

                    let idx_val = vm_try!(vm_read(regs, *idx, num_locals));
                    // Slice key: tuple of (lo, hi, step) produced by the compiler.
                    if let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
                        let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                        let result = vm_try!(self.eval_slice(obj_val, lo, hi, st));
                        regs[*dst as usize] = result;
                    } else {
                        // Fast path: read directly from the register without cloning
                        // the entire collection (avoids O(n) clone per GetItem call).
                        let result = if let Some(ov) = regs[*obj as usize].as_some() {
                            match ov.kind() {
                                ValueKind::List(items) => {
                                    let i = vm_try!(normalize_index(&idx_val, items.len(), "list"));
                                    Some(items[i].clone())
                                }
                                ValueKind::Tuple(items) => {
                                    let i = vm_try!(normalize_index(&idx_val, items.len(), "tuple"));
                                    Some(items[i].clone())
                                }
                                ValueKind::Dict(dict) => {
                                    let key = vm_try!(idx_val.to_key().ok_or_else(|| {
                                        PyError::Runtime("unhashable key type".to_string())
                                    }));
                                    Some(vm_try!(dict
                                        .get(&key)
                                        .cloned()
                                        .ok_or_else(|| PyError::Runtime("key error".to_string()))))
                                }
                                _ => None,
                            }
                        } else { None };
                        if let Some(r) = result {
                            regs[*dst as usize] = r;
                        } else {
                            let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                            let r = vm_try!(self.eval_index(obj_val, idx_val));
                            regs[*dst as usize] = r;
                        }
                    }
                }
                Insn::SetItem(obj, idx, val) => {
                    let idx_val = vm_try!(vm_read(regs, *idx, num_locals));
                    let val_val = vm_try!(vm_read(regs, *val, num_locals));
                    // Slice assignment: tuple key on a list.
                    if let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
                        let new_items: Vec<Value> = match val_val.kind() {
                            ValueKind::List(v) => v.clone(),
                            _ => vm_try!(iter_values(val_val).map_err(|_| {
                                PyError::Runtime("slice assignment requires iterable".to_string())
                            })),
                        };
                        let list_mut = if let Some(ov) = regs[*obj as usize].as_some_mut() {
                            ov.as_list_mut()
                        } else { None };
                        if let Some(items) = list_mut {
                            vm_try!(Self::slice_setitem(
                                items,
                                lo.as_ref(),
                                hi.as_ref(),
                                st.as_ref(),
                                new_items,
                            ));
                        } else {
                            vm_try!(Err(PyError::Runtime(
                                "object does not support slice assignment".to_string(),
                            )));
                        }
                    } else {
                        // Non-slice set: determine target type first, then mutate
                        let target_kind = regs[*obj as usize].as_some().map(|v| match v.kind() {
                            ValueKind::List(_) => 1u8,
                            ValueKind::Dict(_) => 2u8,
                            ValueKind::PyInstance(_) => 3u8,
                            _ => 0u8,
                        }).unwrap_or(0);
                        match target_kind {
                            1 => {
                                let items = vm_try!(expect_list_mut(regs, *obj, "SetItem"));
                                let i = vm_try!(normalize_index(&idx_val, items.len(), "list"));
                                items[i] = val_val;
                            }
                            2 => {
                                let dict = vm_try!(expect_dict_mut(regs, *obj, "SetItem"));
                                let key = vm_try!(idx_val.to_key().ok_or_else(|| {
                                    PyError::Runtime("unhashable type".to_string())
                                }));
                                dict.insert(key, val_val);
                            }
                            3 => {
                                let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                                if let ValueKind::PyInstance(inst) = obj_val.kind() {
                                    let inst_rc = Rc::clone(inst);
                                    let class = Rc::clone(&inst_rc.borrow().class);
                                    if let Some(method_val) = lookup_class_attr(&class, "__setitem__")
                                        && let ValueKind::UserFunction(f) = method_val.kind() {
                                            let func = Rc::clone(f);
                                            vm_try!(self.call_user_function_expanded(
                                                func,
                                                &[
                                                    ExpandedCallArg { name: None, value: idx_val },
                                                    ExpandedCallArg { name: None, value: val_val },
                                                ],
                                                &[Value::py_instance(inst_rc)],
                                            ));
                                            continue;
                                        }
                                }
                                vm_try!(Err(PyError::Named(
                                    "TypeError".to_string(),
                                    "object does not support item assignment".to_string(),
                                )));
                            }
                            _ => {
                                vm_try!(Err(PyError::Runtime(
                                    "object does not support item assignment".to_string(),
                                )));
                            }
                        }
                    }
                }
                Insn::DeleteItem(obj, idx) => {
                    let idx_val = vm_try!(vm_read(regs, *idx, num_locals));
                    if let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
                        let list_mut = if let Some(ov) = regs[*obj as usize].as_some_mut() {
                            ov.as_list_mut()
                        } else { None };
                        if let Some(items) = list_mut {
                            vm_try!(Self::slice_delitem(
                                items,
                                lo.as_ref(),
                                hi.as_ref(),
                                st.as_ref(),
                            ));
                        } else {
                            vm_try!(Err(PyError::Runtime(
                                "object does not support slice deletion".to_string(),
                            )));
                        }
                    } else {
                        let mut handled = false;
                        if let Some(ov) = regs[*obj as usize].as_some_mut() {
                            if let Some(items) = ov.as_list_mut() {
                                let i = vm_try!(normalize_index(&idx_val, items.len(), "list"));
                                if i == items.len() - 1 {
                                    items.pop();
                                } else {
                                    items.remove(i);
                                }
                                handled = true;
                            } else if let Some(dict) = ov.as_dict_mut() {
                                let key = vm_try!(idx_val.to_key().ok_or_else(|| {
                                    PyError::Runtime("unhashable type".to_string())
                                }));
                                dict.shift_remove(&key);
                                handled = true;
                            }
                        }
                        if !handled {
                            // Try __delitem__ on user-defined instances.
                            let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                            if let ValueKind::PyInstance(inst) = obj_val.kind() {
                                let inst_rc = Rc::clone(inst);
                                let class = Rc::clone(&inst_rc.borrow().class);
                                if let Some(method_val) = lookup_class_attr(&class, "__delitem__")
                                    && let ValueKind::UserFunction(f) = method_val.kind() {
                                        let func = Rc::clone(f);
                                        vm_try!(self.call_user_function_expanded(
                                            func,
                                            &[ExpandedCallArg { name: None, value: idx_val }],
                                            &[Value::py_instance(inst_rc)],
                                        ));
                                        continue;
                                    }
                                let class_name = class.borrow().name.clone();
                                vm_try!(Err(PyError::Named(
                                    "TypeError".to_string(),
                                    format!("'{class_name}' object doesn't support item deletion"),
                                )));
                            }
                            vm_try!(Err(PyError::Named(
                                "TypeError".to_string(),
                                "object does not support item deletion".to_string(),
                            )));
                        }
                    }
                }
                Insn::DeleteName(name_idx) => {
                    let name = pool_get!(code.names, *name_idx, "name").clone();
                    self.env.borrow_mut().values.remove(&name);
                }
                Insn::DeleteLocal(reg) => {
                    regs[*reg as usize] = Value::unset();
                }

                // ── Control flow ─────────────────────────────────────────
                Insn::Jump(offset) => {
                    pc = jump_pc!(*offset);
                }
                Insn::JumpIfFalse(cond, offset) => {
                    let fast = if let Some(cv) = regs[*cond as usize].as_some() {
                        match cv.kind() {
                            ValueKind::Int(n)  => { if n == 0 { pc = jump_pc!(*offset); } true }
                            ValueKind::Bool(b) => { if !b    { pc = jump_pc!(*offset); } true }
                            _ => false,
                        }
                    } else { false };
                    if fast { continue; }
                    let cond_val = vm_try!(vm_read(regs, *cond, num_locals));
                    if !vm_try!(self.truthy_value(&cond_val)) {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::JumpIfTrue(cond, offset) => {
                    let fast = if let Some(cv) = regs[*cond as usize].as_some() {
                        match cv.kind() {
                            ValueKind::Int(n)  => { if n != 0 { pc = jump_pc!(*offset); } true }
                            ValueKind::Bool(b) => { if b      { pc = jump_pc!(*offset); } true }
                            _ => false,
                        }
                    } else { false };
                    if fast { continue; }
                    let cond_val = vm_try!(vm_read(regs, *cond, num_locals));
                    if vm_try!(self.truthy_value(&cond_val)) {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::CmpJumpIfFalse(lhs, op, rhs, offset) => {
                    let lv = &regs[*lhs as usize];
                    let rv = &regs[*rhs as usize];
                    if let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), rv.kind())
                        && let Some(cond) = int_cmp(a, b, *op) {
                                if !cond { pc = jump_pc!(*offset); }
                                continue;
                            }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
                    if !vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = jump_pc!(*offset); }
                }
                Insn::CmpJumpIfTrue(lhs, op, rhs, offset) => {
                    let lv = &regs[*lhs as usize];
                    let rv = &regs[*rhs as usize];
                    if let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), rv.kind())
                        && let Some(cond) = int_cmp(a, b, *op) {
                                if cond { pc = jump_pc!(*offset); }
                                continue;
                            }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
                    if vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = jump_pc!(*offset); }
                }
                Insn::CmpJumpIfFalseConst(lhs, op, const_idx, offset) => {
                    let cv = pool_get!(code.consts, *const_idx, "const");
                    if let Some(lv) = regs[*lhs as usize].as_some()
                        && let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), cv.kind())
                            && let Some(cond) = int_cmp(a, b, *op) {
                                if !cond { pc = jump_pc!(*offset); }
                                continue;
                            }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = cv.clone();
                    if !vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = jump_pc!(*offset); }
                }
                Insn::CmpJumpIfTrueConst(lhs, op, const_idx, offset) => {
                    let cv = pool_get!(code.consts, *const_idx, "const");
                    if let Some(lv) = regs[*lhs as usize].as_some()
                        && let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), cv.kind())
                            && let Some(cond) = int_cmp(a, b, *op) {
                                if cond { pc = jump_pc!(*offset); }
                                continue;
                            }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = cv.clone();
                    if vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = jump_pc!(*offset); }
                }

                // ── Exception handling ───────────────────────────────────
                Insn::SetupExcept(offset) => {
                    exc_handlers.push(jump_pc!(*offset));
                }
                Insn::PopExcept => {
                    exc_handlers.pop();
                }
                Insn::LoadExc(dst) => {
                    let exc = vm_try!(self.active_exception.clone().ok_or_else(|| {
                        PyError::Runtime("no active exception".to_string())
                    }));
                    regs[*dst as usize] = exc;
                }
                Insn::MatchExcept(type_reg, offset) => {
                    let type_val = vm_try!(vm_read(regs, *type_reg, num_locals));
                    let exc = vm_try!(self.active_exception.clone().ok_or_else(|| {
                        PyError::Runtime(
                            "internal error: MatchExcept with no active exception".to_string(),
                        )
                    }));
                    if !vm_try!(self.exception_matches(&exc, &type_val)) {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::EndExcept => {
                    self.active_exception = None;
                }
                Insn::RaiseAssert(msg_reg) => {
                    let msg = vm_try!(vm_read(regs, *msg_reg, num_locals));
                    let msg_str = if msg.is_none() {
                        String::new()
                    } else {
                        msg.to_py_str()
                    };
                    let exc =
                        vm_try!(self.instantiate_named_exception("AssertionError", msg_str));
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }
                Insn::RaiseValue(src) => {
                    let val = vm_try!(vm_read(regs, *src, num_locals));
                    let exc = vm_try!(self.coerce_to_exception(val));
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }
                Insn::RaiseFrom(src, cause_reg) => {
                    let val = vm_try!(vm_read(regs, *src, num_locals));
                    let cause = vm_try!(vm_read(regs, *cause_reg, num_locals));
                    let exc = vm_try!(self.coerce_to_exception(val));
                    if let ValueKind::PyInstance(inst) = exc.kind() {
                        inst.borrow_mut().attrs.insert("__cause__".to_string(), cause);
                        inst.borrow_mut().attrs.insert("__suppress_context__".to_string(), Value::bool_(true));
                    }
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }
                Insn::RaiseReRaise => {
                    let exc = vm_try!(self.active_exception.clone().ok_or_else(|| {
                        PyError::Runtime("no active exception to re-raise".to_string())
                    }));
                    vm_try!(Err::<(), _>(PyError::Raised(exc)));
                }

                // ── Calls ────────────────────────────────────────────────
                Insn::Call(func_reg, argc) => {
                    let func_val = vm_try!(vm_read(regs, *func_reg, num_locals));
                    // Fast path for id(x): read the pool pointer directly from the
                    // register without cloning.  Cloning a list/tuple/str creates a
                    // new allocation, so the pointer seen inside call_function_expanded
                    // would differ from the original object's address.
                    if *argc == 1
                        && let ValueKind::BuiltinFunction("id") = func_val.kind() {
                            let maybe_id: Option<i64> = regs
                                .get((*func_reg + 1) as usize)
                                .and_then(|v| v.value_id());
                            if let Some(id_val) = maybe_id {
                                regs[*func_reg as usize] = Value::int(id_val);
                                continue 'vm;
                            }
                        }
                    // Reuse the interpreter-level buffer to avoid a per-call heap
                    // allocation in the common (non-recursive) case.
                    let mut buf = std::mem::take(&mut self.call_arg_buf);
                    buf.clear();
                    for i in 0..crate::bytecode::Reg::from(*argc) {
                        buf.push(ExpandedCallArg {
                            name: None,
                            value: vm_try!(vm_read(regs, *func_reg + 1 + i, num_locals)),
                        });
                    }
                    let call_result = self.call_function_expanded(func_val, &buf);
                    self.call_arg_buf = buf;
                    regs[*func_reg as usize] = vm_try!(call_result);
                }

                Insn::CallMemo(func_reg, argc) => {
                    // Cache-first path for known-pure callees.
                    let is_pure_fn = if let Some(fv) = regs[*func_reg as usize].as_some() {
                        if let ValueKind::UserFunction(func) = fv.kind() {
                            func.is_pure
                        } else { false }
                    } else { false };

                    if is_pure_fn
                        && let Some(fv) = regs[*func_reg as usize].as_some()
                            && let ValueKind::UserFunction(func) = fv.kind() {
                                let fn_id = func.id;
                                let mut key = std::mem::take(&mut self.key_scratch);
                                key.clear();
                                let mut all_hashable = true;
                                for i in 0..*argc as usize {
                                    match regs[*func_reg as usize + 1 + i]
                                        .to_key()
                                    {
                                        Some(k) => key.push(k),
                                        None => {
                                            all_hashable = false;
                                            break;
                                        }
                                    }
                                }
                                if all_hashable {
                                    let lookup = (fn_id, key);
                                    let hit = self.fn_cache.get(&lookup).cloned();
                                    let (_, key) = lookup;
                                    self.key_scratch = key;
                                    if let Some(cached) = hit {
                                        regs[*func_reg as usize] = cached;
                                        continue;
                                    }
                                } else {
                                    self.key_scratch = key;
                                }
                            }
                    // Cache miss or unhashable args: normal call (call_function_expanded
                    // will store the result in fn_cache on the way back).
                    let func_val = vm_try!(vm_read(regs, *func_reg, num_locals));
                    let mut buf = std::mem::take(&mut self.call_arg_buf);
                    buf.clear();
                    for i in 0..crate::bytecode::Reg::from(*argc) {
                        buf.push(ExpandedCallArg {
                            name: None,
                            value: vm_try!(vm_read(regs, *func_reg + 1 + i, num_locals)),
                        });
                    }
                    let call_result = self.call_function_expanded(func_val, &buf);
                    self.call_arg_buf = buf;
                    regs[*func_reg as usize] = vm_try!(call_result);
                }

                Insn::CallMethod { dst, obj, name_idx, args_base, nargs } => {
                    let r = self.exec_call_method(regs, num_locals, *dst, *obj, *name_idx, *args_base, *nargs, code);
                    regs[*dst as usize] = vm_try!(r);
                }

                Insn::CallMethodExpanded { dst, obj, name_idx, pos_list, kw_dict } => {
                    let r = self.exec_call_method_expanded(regs, num_locals, *dst, *obj, *name_idx, *pos_list, *kw_dict, code);
                    regs[*dst as usize] = vm_try!(r);
                }

                // ── Returns ──────────────────────────────────────────────
                Insn::Return(src) => {
                    return Ok(vm_try!(vm_read(regs, *src, num_locals)));
                }
                Insn::ReturnNone => {
                    return Ok(Value::none());
                }

                // ── Tail-call ────────────────────────────────────────────
                Insn::TailCall { args_base, nargs } => {
                    // The function to call lives at func_reg = args_base - 1.
                    let func_reg = args_base - 1;
                    let callee_val = vm_try!(vm_read(regs, func_reg, num_locals));

                    // Self-call check: if the callee is the same user function as
                    // the one currently executing, and we are not inside a try
                    // block (exc_handlers must be empty for safe frame reuse),
                    // reset the register file and loop back to pc=0.
                    let is_self_call = if let Some(fn_id) = current_fn_id {
                        match callee_val.kind() {
                            ValueKind::UserFunction(f) => f.id == fn_id,
                            _ => false,
                        }
                    } else {
                        false
                    };

                    if is_self_call && exc_handlers.is_empty() {
                        // Guard against infinite tail recursion: treat each
                        // TCO iteration as one "call depth" unit.  This allows
                        // factorial(MAX_CALL_DEPTH * 100) while still raising
                        // RecursionError for truly infinite self-tail-calls.
                        tco_iters += 1;
                        if tco_iters > MAX_CALL_DEPTH * 100 {
                            let exc = vm_try!(self.instantiate_named_exception(
                                "RecursionError",
                                "maximum recursion depth exceeded".to_string(),
                            ));
                            return Err(PyError::Raised(exc));
                        }
                        // Collect new argument values before we overwrite any registers.
                        let mut new_args: Vec<Value> =
                            Vec::with_capacity(*nargs as usize);
                        for i in 0..*nargs as u32 {
                            new_args.push(vm_try!(vm_read(regs, args_base + i, num_locals)));
                        }
                        // Reset all registers to unset.
                        for slot in regs.iter_mut() {
                            *slot = Value::unset();
                        }
                        // Bind new positional args into parameter registers 0..nargs.
                        for (i, arg) in new_args.into_iter().enumerate() {
                            regs[i] = arg;
                        }
                        // Restore the self-reference in its original register so
                        // the recursive body can call itself again.
                        regs[func_reg as usize] = callee_val;
                        // Reset iterator and exception-handler state.
                        for slot in iters.iter_mut() {
                            *slot = None;
                        }
                        // exc_handlers is already empty (checked above).
                        // Jump to the top of the function.
                        pc = 0;
                        continue 'vm;
                    } else {
                        // Fallback: normal call, then return the result.
                        let mut buf = std::mem::take(&mut self.call_arg_buf);
                        buf.clear();
                        for i in 0..*nargs as u32 {
                            buf.push(ExpandedCallArg {
                                name: None,
                                value: vm_try!(vm_read(regs, args_base + i, num_locals)),
                            });
                        }
                        let call_result = self.call_function_expanded(callee_val, &buf);
                        self.call_arg_buf = buf;
                        return Ok(vm_try!(call_result));
                    }
                }

                // ── Collection builders ──────────────────────────────────
                Insn::BuildList(dst, base, n) => {
                    let mut items: Vec<Value> = Vec::with_capacity(*n as usize);
                    for i in 0..crate::bytecode::Reg::from(*n) {
                        items.push(vm_try!(vm_read(regs, *base + i, num_locals)));
                    }
                    regs[*dst as usize] = Value::list(items);
                }
                Insn::BuildTuple(dst, base, n) => {
                    let mut items = Vec::with_capacity(*n as usize);
                    for i in 0..crate::bytecode::Reg::from(*n) {
                        items.push(vm_try!(vm_read(regs, *base + i, num_locals)));
                    }
                    regs[*dst as usize] = Value::tuple(items);
                }
                Insn::BuildDict(dst, base, n) => {
                    let mut dict = indexmap::IndexMap::new();
                    for i in 0..crate::bytecode::Reg::from(*n) {
                        let k_val = vm_try!(vm_read(regs, *base + i * 2, num_locals));
                        let v_val = vm_try!(vm_read(regs, *base + i * 2 + 1, num_locals));
                        let key = vm_try!(k_val.to_key().ok_or_else(|| {
                            PyError::Runtime("unhashable type in dict key".to_string())
                        }));
                        dict.insert(key, v_val);
                    }
                    regs[*dst as usize] = Value::dict(dict);
                }
                Insn::SetAdd(set_reg, val_reg) => {
                    let val = vm_try!(vm_read(regs, *val_reg, num_locals));
                    let key = vm_try!(val.to_key().ok_or_else(|| PyError::Runtime(
                        "unhashable type in set comprehension".to_string()
                    )));
                    let set = vm_try!(regs[*set_reg as usize]
                        .as_set_mut()
                        .ok_or_else(|| PyError::Runtime("SetAdd: not a set".to_string())));
                    set.insert(key);
                }
                Insn::ListAppend(list_reg, val_reg) => {
                    let val = vm_try!(vm_read(regs, *val_reg, num_locals));
                    let items = vm_try!(expect_list_mut(regs, *list_reg, "ListAppend"));
                    items.push(val);
                }
                Insn::ListExtend(list_reg, src_reg) => {
                    let src_val = vm_try!(vm_read(regs, *src_reg, num_locals));
                    let items_to_add = vm_try!(iter_values(src_val));
                    let items = vm_try!(expect_list_mut(regs, *list_reg, "ListExtend"));
                    items.extend(items_to_add);
                }
                Insn::DictUpdate(dict_reg, src_reg) => {
                    let src_val = vm_try!(vm_read(regs, *src_reg, num_locals));
                    let src_dict = match src_val.kind() {
                        ValueKind::Dict(d) => d.clone(),
                        _ => vm_try!(Err(PyError::Runtime(
                            "DictUpdate requires a dict argument".to_string(),
                        ))),
                    };
                    let dict = vm_try!(expect_dict_mut(regs, *dict_reg, "DictUpdate"));
                    for (k, v) in src_dict {
                        dict.insert(k, v);
                    }
                }

                // ── Generator yield ──────────────────────────────────────
                Insn::Yield { src, dst } => {
                    // Suspend the generator.  pc has already been incremented
                    // past this instruction, so resumption continues at pc.
                    let yielded = vm_try!(vm_read(regs, *src, num_locals));
                    // Pre-fill dst with None: this is the sent value that the
                    // yield expression evaluates to on resumption.  Proper
                    // send() support would overwrite this in resume_generator.
                    regs[*dst as usize] = Value::none();
                    // Save current iters/exc_handlers/pc to the thread-local
                    // so that resume_generator() can write them back into the
                    // GeneratorFrame after we unwind.
                    GEN_SAVE.with(|cell| {
                        *cell.borrow_mut() = Some((
                            iters.clone(),
                            exc_handlers.clone(),
                            pc, // already past the Yield instruction
                        ));
                    });
                    return Err(PyError::GeneratorYield(yielded));
                }

                // ── Unpack ───────────────────────────────────────────────
                Insn::Unpack(base, src, n) => {
                    let src_val = vm_try!(vm_read(regs, *src, num_locals));
                    let items = vm_try!(iter_values(src_val));
                    if items.len() < *n as usize {
                        vm_try!(Err::<(), _>(PyError::Runtime(format!(
                            "not enough values to unpack (expected {}, got {})",
                            n,
                            items.len()
                        ))));
                    } else if items.len() > *n as usize {
                        vm_try!(Err::<(), _>(PyError::Runtime(format!(
                            "too many values to unpack (expected {})",
                            n
                        ))));
                    }
                    for (i, v) in items.into_iter().enumerate() {
                        let dst = *base as usize + i;
                        if dst >= regs.len() {
                            vm_try!(Err(PyError::Runtime(format!(
                                "Unpack: register {dst} out of range"
                            ))));
                        }
                        regs[dst] = v;
                    }
                }

                Insn::UnpackEx { src, before, after, dst_base } => {
                    let src_val = vm_try!(vm_read(regs, *src, num_locals));
                    let items = vm_try!(iter_values(src_val));
                    let before = *before as usize;
                    let after = *after as usize;
                    let min_len = before + after;
                    if items.len() < min_len {
                        vm_try!(Err::<(), _>(PyError::Named(
                            "ValueError".to_string(),
                            format!(
                                "not enough values to unpack (expected at least {}, got {})",
                                min_len,
                                items.len()
                            ),
                        )));
                    }
                    let base = *dst_base as usize;
                    // First `before` elements
                    for i in 0..before {
                        let dst = base + i;
                        if dst >= regs.len() {
                            vm_try!(Err(PyError::Runtime(format!(
                                "UnpackEx: register {dst} out of range"
                            ))));
                        }
                        regs[dst] = items[i].clone();
                    }
                    // Middle as a list → R[base + before]
                    let star_end = items.len() - after;
                    let middle: Vec<Value> = items[before..star_end].to_vec();
                    let star_dst = base + before;
                    if star_dst >= regs.len() {
                        vm_try!(Err(PyError::Runtime(format!(
                            "UnpackEx: register {star_dst} out of range"
                        ))));
                    }
                    regs[star_dst] = Value::list(middle);
                    // Last `after` elements
                    for i in 0..after {
                        let dst = base + before + 1 + i;
                        if dst >= regs.len() {
                            vm_try!(Err(PyError::Runtime(format!(
                                "UnpackEx: register {dst} out of range"
                            ))));
                        }
                        regs[dst] = items[star_end + i].clone();
                    }
                }

                // ── Iterator ─────────────────────────────────────────────
                Insn::GetIter(slot, src) => {
                    // Range: lazy counter — no Vec needed.
                    // List/Tuple in a LOCAL register: lazy index — avoids the O(n)
                    //   upfront clone; the local slot is stable for the function lifetime.
                    // Temp register or other type: materialise immediately, because
                    //   a temp reg is freed and may be overwritten by the loop body.
                    let is_list_or_tuple_local = if *src < num_locals {
                        if let Some(v) = regs[*src as usize].as_some() {
                            matches!(v.kind(), ValueKind::List(_) | ValueKind::Tuple(_))
                        } else { false }
                    } else { false };

                    let state = if is_list_or_tuple_local {
                        IterState::Indexed { reg: *src, pos: 0 }
                    } else {
                        let src_val = vm_try!(vm_read(regs, *src, num_locals));
                        match src_val.kind() {
                            ValueKind::Range { start, stop, step } => {
                                if step == 0 {
                                    vm_try!(Err(PyError::Named(
                                        "ValueError".to_string(),
                                        "range() arg 3 must not be zero".to_string(),
                                    )));
                                }
                                IterState::Range { cur: start, stop, step }
                            }
                            ValueKind::Enumerate { source, start } => {
                                let items = vm_try!(iter_values(source.clone()));
                                IterState::Enumerate { items, pos: 0, counter: start }
                            }
                            ValueKind::Zip { sources } => {
                                if sources.is_empty() {
                                    IterState::Materialized(vec![], 0)
                                } else {
                                    let mut vecs: Vec<Vec<Value>> = Vec::with_capacity(sources.len());
                                    for s in sources {
                                        vecs.push(vm_try!(iter_values(s.clone())));
                                    }
                                    let len = vecs.iter().map(|v| v.len()).min().unwrap_or(0);
                                    IterState::Zip { sources: vecs, pos: 0, len }
                                }
                            }
                            ValueKind::Reversed { source } => {
                                let items = vm_try!(iter_values(source.clone()));
                                let len = items.len();
                                IterState::ReversedItems { items, pos: len }
                            }
                            ValueKind::Generator(_) => {
                                // A generator is its own iterator.
                                IterState::UserDefined(src_val)
                            }
                            ValueKind::PyInstance(inst) => {
                                let inst_rc = Rc::clone(inst);
                                let class = Rc::clone(&inst_rc.borrow().class);
                                if let Some(method_val) = lookup_class_attr(&class, "__iter__") {
                                    if let ValueKind::UserFunction(f) = method_val.kind() {
                                        let func = Rc::clone(f);
                                        let iter_obj = vm_try!(self.call_user_function_expanded(
                                            func,
                                            &[],
                                            &[Value::py_instance(inst_rc)],
                                        ));
                                        IterState::UserDefined(iter_obj)
                                    } else {
                                        vm_try!(Err(PyError::Named(
                                            "TypeError".to_string(),
                                            "__iter__ is not callable".to_string(),
                                        )));
                                        unreachable!()
                                    }
                                } else {
                                    // No __iter__: try to materialise via iter_values (will fail)
                                    IterState::Materialized(vm_try!(iter_values(src_val)), 0)
                                }
                            }
                            _ => {
                                IterState::Materialized(vm_try!(iter_values(src_val)), 0)
                            }
                        }
                    };
                    iters[*slot as usize] = Some(state);
                }
                Insn::ForIter(dst, slot, offset) => {
                    #[allow(clippy::collapsible_match)]
                    match iters[*slot as usize].as_mut() {
                        // Hot path: indexed iteration over a list/tuple held in a register.
                        // Direct as_list()/as_tuple() accessors skip the kind() decode and
                        // the big ValueKind match that the old implementation went through
                        // on every iteration.
                        Some(IterState::Indexed { reg, pos }) => {
                            let src = *reg as usize;
                            let cur_pos = *pos;
                            let items: Option<&Vec<Value>> = if regs[src].is_unset() {
                                None
                            } else {
                                regs[src].as_list().or_else(|| regs[src].as_tuple())
                            };
                            match items {
                                Some(items) if cur_pos < items.len() => {
                                    // SAFETY: cur_pos < items.len() checked just above.
                                    let v = unsafe { items.get_unchecked(cur_pos).clone() };
                                    *pos = cur_pos + 1;
                                    regs[*dst as usize] = v;
                                }
                                _ => pc = jump_pc!(*offset),
                            }
                        }
                        Some(IterState::Materialized(items, pos)) => {
                            let cur_pos = *pos;
                            if cur_pos < items.len() {
                                // SAFETY: cur_pos < items.len() checked just above.
                                let v = unsafe { items.get_unchecked(cur_pos).clone() };
                                *pos = cur_pos + 1;
                                regs[*dst as usize] = v;
                            } else {
                                pc = jump_pc!(*offset);
                            }
                        }
                        Some(IterState::Range { cur, stop, step }) => {
                            let exhausted =
                                if *step > 0 { *cur >= *stop } else { *cur <= *stop };
                            if exhausted {
                                pc = jump_pc!(*offset);
                            } else {
                                let v = Value::int(*cur);
                                *cur += *step;
                                regs[*dst as usize] = v;
                            }
                        }
                        Some(IterState::Enumerate { items, pos, counter }) => {
                            if *pos < items.len() {
                                let v = Value::tuple(vec![Value::int(*counter), items[*pos].clone()]);
                                *pos += 1;
                                *counter += 1;
                                regs[*dst as usize] = v;
                            } else {
                                pc = jump_pc!(*offset);
                            }
                        }
                        Some(IterState::Zip { sources, pos, len }) => {
                            if *pos < *len {
                                let row: Vec<Value> = sources.iter().map(|s| s[*pos].clone()).collect();
                                *pos += 1;
                                regs[*dst as usize] = Value::tuple(row);
                            } else {
                                pc = jump_pc!(*offset);
                            }
                        }
                        Some(IterState::ReversedItems { items, pos }) => {
                            if *pos > 0 {
                                *pos -= 1;
                                let v = items[*pos].clone();
                                regs[*dst as usize] = v;
                            } else {
                                pc = jump_pc!(*offset);
                            }
                        }
                        Some(IterState::UserDefined(iter_obj)) => {
                            // Call __next__() on the iterator object; stop on StopIteration.
                            let iter_val = iter_obj.clone();
                            let next_result: Option<Result<Value>> =
                                if let ValueKind::Generator(state_rc) = iter_val.kind() {
                                    let state_rc = Rc::clone(state_rc);
                                    let mut borrow = state_rc.borrow_mut();
                                    if let Some(native) = borrow.downcast_mut::<NativeIterFrame>() {
                                        // Built-in iterator created by iter().
                                        if native.pos >= native.items.len() {
                                            Some(Err(PyError::Named(
                                                "StopIteration".to_string(),
                                                String::new(),
                                            )))
                                        } else {
                                            let item = native.items[native.pos].clone();
                                            native.pos += 1;
                                            Some(Ok(item))
                                        }
                                    } else if let Some(frame) = borrow.downcast_mut::<GeneratorFrame>() {
                                        // Resume the generator.
                                        if frame.done {
                                            Some(Err(PyError::Named(
                                                "StopIteration".to_string(),
                                                String::new(),
                                            )))
                                        } else {
                                            Some(self.resume_generator(frame))
                                        }
                                    } else {
                                        Some(Err(PyError::Runtime(
                                            "invalid generator state".to_string(),
                                        )))
                                    }
                                } else if let ValueKind::PyInstance(inst) = iter_val.kind() {
                                    let inst_rc = Rc::clone(inst);
                                    let class = Rc::clone(&inst_rc.borrow().class);
                                    if let Some(method_val) = lookup_class_attr(&class, "__next__") {
                                        if let ValueKind::UserFunction(f) = method_val.kind() {
                                            let func = Rc::clone(f);
                                            Some(self.call_user_function_expanded(
                                                func,
                                                &[],
                                                &[Value::py_instance(inst_rc)],
                                            ))
                                        } else { None }
                                    } else { None }
                                } else { None };
                            match next_result {
                                Some(Ok(val)) => {
                                    regs[*dst as usize] = val;
                                }
                                Some(Err(PyError::GeneratorYield(val))) => {
                                    regs[*dst as usize] = val;
                                }
                                Some(Err(PyError::Raised(exc))) => {
                                    // Check for StopIteration: covers both `raise StopIteration()`
                                    // (PyInstance) and bare `raise StopIteration` (PyClass).
                                    let is_stop = match exc.kind() {
                                        ValueKind::PyInstance(inst) => {
                                            inst.borrow().class.borrow().name == "StopIteration"
                                        }
                                        ValueKind::PyClass(cls) => {
                                            cls.borrow().name == "StopIteration"
                                        }
                                        _ => false,
                                    };
                                    if is_stop {
                                        pc = jump_pc!(*offset);
                                    } else {
                                        vm_try!(Err(PyError::Raised(exc)));
                                    }
                                }
                                Some(Err(PyError::Named(ref cls, _)))
                                    if cls == "StopIteration" =>
                                {
                                    pc = jump_pc!(*offset);
                                }
                                Some(Err(e)) => { vm_try!(Err(e)); }
                                None => {
                                    vm_try!(Err(PyError::Named(
                                        "TypeError".to_string(),
                                        "iterator has no __next__ method".to_string(),
                                    )));
                                }
                            }
                        }
                        None => {
                            pc = jump_pc!(*offset);
                        }
                    }
                }
                Insn::ForCountReg(var, cmp_op, stop_reg, step_idx, offset) => {
                    let step = match pool_get!(code.consts, *step_idx, "const").kind() {
                        ValueKind::Int(s) => s,
                        _ => unreachable!("ForCountReg step must be Int"),
                    };
                    let fast = {
                        let vv = &regs[*var as usize];
                        let sv = &regs[*stop_reg as usize];
                        if let (ValueKind::Int(cur), ValueKind::Int(stop)) = (vv.kind(), sv.kind()) {
                            let next = cur.wrapping_add(step);
                            let cont = match cmp_op {
                                BinaryOp::Lt => next < stop,
                                BinaryOp::Gt => next > stop,
                                _ => unreachable!("ForCountReg uses Lt or Gt only"),
                            };
                            if cont { regs[*var as usize] = Value::int(next); }
                            else { pc = jump_pc!(*offset); }
                            true
                        } else { false }
                    };
                    if !fast {
                        vm_try!(Err(crate::error::PyError::Runtime(
                            "for-range: non-integer counter or stop".into(),
                        )));
                    }
                }
                Insn::ForCountConst(var, cmp_op, stop_idx, step_idx, offset) => {
                    let step = match pool_get!(code.consts, *step_idx, "const").kind() {
                        ValueKind::Int(s) => s,
                        _ => unreachable!("ForCountConst step must be Int"),
                    };
                    let stop = match pool_get!(code.consts, *stop_idx, "const").kind() {
                        ValueKind::Int(s) => s,
                        _ => unreachable!("ForCountConst stop must be Int"),
                    };
                    let fast = if let Some(vv) = regs[*var as usize].as_some() {
                        if let ValueKind::Int(cur) = vv.kind() {
                            let next = cur.wrapping_add(step);
                            let cont = match cmp_op {
                                BinaryOp::Lt => next < stop,
                                BinaryOp::Gt => next > stop,
                                _ => unreachable!("ForCountConst uses Lt or Gt only"),
                            };
                            if cont { regs[*var as usize] = Value::int(next); }
                            else { pc = jump_pc!(*offset); }
                            true
                        } else { false }
                    } else { false };
                    if !fast {
                        vm_try!(Err(crate::error::PyError::Runtime(
                            "for-range: non-integer counter".into(),
                        )));
                    }
                }
                Insn::ForCountConstInline(var, cmp_op, stop, step, offset) => {
                    // Same semantics as ForCountConst but with stop/step inlined; no
                    // per-iteration consts-pool lookup, no .kind() decode for them.
                    let step = *step as i64;
                    let stop = *stop as i64;
                    let fast = if let Some(vv) = regs[*var as usize].as_some() {
                        if let ValueKind::Int(cur) = vv.kind() {
                            let next = cur.wrapping_add(step);
                            let cont = match cmp_op {
                                BinaryOp::Lt => next < stop,
                                BinaryOp::Gt => next > stop,
                                _ => unreachable!("ForCountConstInline uses Lt or Gt only"),
                            };
                            if cont { regs[*var as usize] = Value::int(next); }
                            else { pc = jump_pc!(*offset); }
                            true
                        } else { false }
                    } else { false };
                    if !fast {
                        vm_try!(Err(crate::error::PyError::Runtime(
                            "for-range: non-integer counter".into(),
                        )));
                    }
                }
                Insn::CheckLocal(reg, name_idx) => {
                    // is_unset() checks for the slot sentinel (uninitialised
                    // local), not for Python's None — Value::is_none() would
                    // mis-fire on legitimate `x = None` followed by a read.
                    if regs[*reg as usize].is_unset() {
                        let name = pool_get!(code.names, *name_idx, "name");
                        vm_try!(Err::<(), _>(crate::error::PyError::Runtime(format!(
                            "cannot access local variable '{}' where it is not associated with a value",
                            name
                        ))));
                    }
                }

                // ── Function / Class creation ────────────────────────────
                Insn::MakeFunction(dst, proto_idx, defs_base, _defs_n) => {
                    let proto = pool_get!(code.fn_protos, *proto_idx, "fn_proto");
                    // Rc bumps only — no Vec clones for param metadata or local_names.
                    let proto_code = Rc::clone(&proto.code);
                    let proto_name = proto.name.clone();
                    let proto_local_index = Rc::clone(&proto.local_index);
                    let proto_local_names = Rc::clone(&proto.local_names);
                    let proto_global_names = Rc::clone(&proto.global_names);
                    let proto_nonlocal_names = Rc::clone(&proto.nonlocal_names);
                    let param_spec = Rc::clone(&proto.param_spec);
                    let is_pure = proto.is_pure;

                    let mut params = Vec::with_capacity(param_spec.names.len());
                    let mut def_slot = 0u32;
                    for i in 0..param_spec.names.len() {
                        let default = if param_spec.has_default[i] {
                            let v =
                                vm_try!(vm_read(regs, *defs_base + def_slot, num_locals));
                            def_slot += 1;
                            Some(v)
                        } else {
                            None
                        };
                        params.push(UserFunctionParam {
                            name: param_spec.names[i].clone(),
                            default,
                            is_args: param_spec.is_args[i],
                            is_kwargs: param_spec.is_kwargs[i],
                            is_keyword_only: param_spec.is_keyword_only[i],
                        });
                    }
                    // Validate that every nonlocal name resolves to an enclosing local scope.
                    for name in proto_nonlocal_names.iter() {
                        if !has_local_binding_in_current_or_ancestor(&self.env, name) {
                            let err = PyError::Runtime(format!(
                                "no binding for nonlocal '{}' found",
                                name
                            ));
                            vm_try!(Err(err));
                        }
                    }
                    let func = Rc::new(UserFunction {
                        id: crate::value::next_fn_id(),
                        name: proto_name,
                        params,
                        local_names: proto_local_names,
                        local_index: proto_local_index,
                        global_names: proto_global_names,
                        nonlocal_names: proto_nonlocal_names,
                        env: Rc::clone(&self.env),
                        is_pure,
                        precompiled_code: Some(proto_code),
                    });
                    regs[*dst as usize] = Value::user_function(func);
                }
                Insn::MakeClass(dst, proto_idx, bases_base, bases_n, name_idx) => {
                    let class_name = pool_get!(code.names, *name_idx, "name").clone();
                    let (class_code, local_index) = {
                        let proto = pool_get!(code.fn_protos, *proto_idx, "fn_proto");
                        (Rc::clone(&proto.code), Rc::clone(&proto.local_index))
                    };
                    let num_class_regs = class_code.num_regs as usize;
                    let mut class_regs: Vec<Value> = vec![Value::unset(); num_class_regs];
                    vm_try!(self.run_bytecode(&class_code, &mut class_regs));
                    let mut attrs = HashMap::new();
                    for (attr_name, &slot) in local_index.iter() {
                        if let Some(v) = class_regs.get(slot as usize)
                            && !v.is_unset() {
                                attrs.insert(attr_name.clone(), v.clone());
                            }
                    }
                    let base = if *bases_n > 0 {
                        let base_val = vm_try!(vm_read(regs, *bases_base, num_locals));
                        match base_val.kind() {
                            ValueKind::PyClass(c) => Some(Rc::clone(c)),
                            _ => {
                                vm_try!(Err::<(), _>(PyError::Runtime(
                                    "class base must be a class".to_string(),
                                )));
                                unreachable!()
                            }
                        }
                    } else {
                        None
                    };
                    let class =
                        Rc::new(RefCell::new(PyClass { name: class_name, base, attrs }));
                    regs[*dst as usize] = Value::py_class(class);
                }

                // ── Import ───────────────────────────────────────────────
                Insn::ImportModule(dst, name_idx) => {
                    let name = pool_get!(code.names, *name_idx, "name").clone();
                    let module = vm_try!(self.load_module(&name));
                    regs[*dst as usize] = module;
                }

                // ── REPL output ──────────────────────────────────────────
                Insn::PrintExpr(src) => {
                    let val = vm_try!(vm_read(regs, *src, num_locals));
                    if !val.is_none() {
                        println!("{}", val.repr());
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn exec_call_method(
        &mut self,
        regs: &mut [Value],
        num_locals: crate::bytecode::Reg,
        _dst: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        args_base: crate::bytecode::Reg,
        nargs: u8,
        code: &crate::bytecode::FnCode,
    ) -> Result<Value> {
        let method = code.names.get(name_idx as usize)
            .ok_or_else(|| PyError::Runtime(format!("bytecode error: name index {name_idx} out of range")))?
            .clone();
        let mut args: Vec<Value> = Vec::with_capacity(nargs as usize);
        for i in 0..crate::bytecode::Reg::from(nargs) {
            args.push(vm_read(regs, args_base + i, num_locals)?);
        }
        // Check if obj is a List, Dict, Tuple, Str, or Set via kind()
        let obj_kind_tag = regs[obj as usize].as_some().map(|v| match v.kind() {
            ValueKind::List(_) => 1u8,
            ValueKind::Dict(_) => 2u8,
            ValueKind::Tuple(_) => 3u8,
            ValueKind::Str(_) => 4u8,
            ValueKind::Set(_) => 5u8,
            _ => 0u8,
        }).unwrap_or(0);

        match obj_kind_tag {
            1 => {
                // as_list_mut is safe: we confirmed tag==List above (single-threaded).
                // obj_reg and dst_reg may coincide; the mutable borrow of the Vec ends
                // before exec_call_method returns, so no alias with the later store.
                let items = regs[obj as usize]
                    .as_list_mut()
                    .ok_or_else(|| PyError::Runtime("internal: expected list".to_string()))?;
                let empty_kw = indexmap::IndexMap::new();
                pyrust_builtins::list::call(&method, items, args, &empty_kw)
            }
            2 => {
                if matches!(method.as_str(), "keys" | "values" | "items") {
                    let rc = regs[obj as usize]
                        .get_dict_rc()
                        .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?
                        .clone();
                    return match method.as_str() {
                        "keys"   => Ok(Value::dict_keys_view(rc)),
                        "values" => Ok(Value::dict_values_view(rc)),
                        "items"  => Ok(Value::dict_items_view(rc)),
                        _ => unreachable!(),
                    };
                }
                let dict = regs[obj as usize]
                    .as_dict_mut()
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                pyrust_builtins::dict::call(&method, dict, args)
            }
            3 => {
                if let Some(ValueKind::Tuple(items)) = regs[obj as usize].as_some().map(|v| v.kind()) {
                    pyrust_builtins::tuple::call(&method, items, args)
                } else {
                    unreachable!()
                }
            }
            4 => {
                if let Some(v) = regs[obj as usize].as_some() {
                    pyrust_builtins::string::call(&method, v, args)
                } else { unreachable!() }
            }
            5 => {
                let set = regs[obj as usize]
                    .as_set_mut()
                    .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                pyrust_builtins::set::call(&method, set, args)
            }
            _ => {
                let obj_val = vm_read(regs, obj, num_locals)?;
                let method_val = self.get_attr(obj_val, &method)?;
                let mut buf = std::mem::take(&mut self.call_arg_buf);
                buf.clear();
                for arg in args {
                    buf.push(ExpandedCallArg { name: None, value: arg });
                }
                let r = self.call_function_expanded(method_val, &buf);
                self.call_arg_buf = buf;
                r
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn exec_call_method_expanded(
        &mut self,
        regs: &mut [Value],
        num_locals: crate::bytecode::Reg,
        _dst: crate::bytecode::Reg,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        pos_list: crate::bytecode::Reg,
        kw_dict: crate::bytecode::Reg,
        code: &crate::bytecode::FnCode,
    ) -> Result<Value> {
        let method = code.names.get(name_idx as usize)
            .ok_or_else(|| PyError::Runtime(format!("bytecode error: name index {name_idx} out of range")))?
            .clone();
        let v = vm_read(regs, pos_list, num_locals)?;
        let pos_items: Vec<Value> = match v.kind() {
            ValueKind::List(items) => items.clone(),
            _ => return Err(PyError::Runtime("CallMethodExpanded: pos_list must be a list".to_string())),
        };
        let v = vm_read(regs, kw_dict, num_locals)?;
        let kw_map = match v.kind() {
            ValueKind::Dict(d) => d.clone(),
            _ => return Err(PyError::Runtime("CallMethodExpanded: kw_dict must be a dict".to_string())),
        };

        let obj_kind_tag = regs[obj as usize].as_some().map(|v| match v.kind() {
            ValueKind::List(_) => 1u8,
            ValueKind::Dict(_) => 2u8,
            ValueKind::Tuple(_) => 3u8,
            ValueKind::Str(_) => 4u8,
            ValueKind::Set(_) => 5u8,
            _ => 0u8,
        }).unwrap_or(0);

        match obj_kind_tag {
            1 => {
                // Intercept list.sort here to support key= (needs interpreter access).
                if method == "sort" {
                    for k in kw_map.keys() {
                        if let PyKey::Str(s) = k
                            && s != "key" && s != "reverse" {
                                return Err(PyError::Named(
                                    "TypeError".to_string(),
                                    format!("sort() got an unexpected keyword argument '{s}'"),
                                ));
                            }
                    }
                    let key_fn = kw_map.get(&PyKey::Str("key".to_string())).cloned();
                    let reverse = kw_map
                        .get(&PyKey::Str("reverse".to_string()))
                        .map(|v| v.truthy())
                        .unwrap_or(false);
                    if let Some(key_fn_val) = key_fn {
                        // Compute keys via the interpreter, then delegate sorting to builtins.
                        let items_snapshot = regs[obj as usize]
                            .as_list()
                            .ok_or_else(|| PyError::Runtime("internal: expected list".to_string()))?
                            .clone();
                        let mut keys: Vec<Value> = Vec::with_capacity(items_snapshot.len());
                        for item in &items_snapshot {
                            let key_val = {
                                let mut buf = std::mem::take(&mut self.call_arg_buf);
                                buf.clear();
                                buf.push(ExpandedCallArg { name: None, value: item.clone() });
                                let r = self.call_function_expanded(key_fn_val.clone(), &buf);
                                self.call_arg_buf = buf;
                                r?
                            };
                            keys.push(key_val);
                        }
                        let items_out = regs[obj as usize]
                            .as_list_mut()
                            .ok_or_else(|| PyError::Runtime("internal: expected list".to_string()))?;
                        return pyrust_builtins::list::sort_with_precomputed_keys(items_out, keys, reverse);
                    }
                    // No key: delegate to builtins (handles reverse kwarg)
                    let items = regs[obj as usize]
                        .as_list_mut()
                        .ok_or_else(|| PyError::Runtime("internal: expected list".to_string()))?;
                    return pyrust_builtins::list::call(&method, items, pos_items, &kw_map);
                }
                let items = regs[obj as usize]
                    .as_list_mut()
                    .ok_or_else(|| PyError::Runtime("internal: expected list".to_string()))?;
                pyrust_builtins::list::call(&method, items, pos_items, &kw_map)
            }
            2 => {
                if matches!(method.as_str(), "keys" | "values" | "items") {
                    let rc = regs[obj as usize]
                        .get_dict_rc()
                        .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?
                        .clone();
                    return match method.as_str() {
                        "keys"   => Ok(Value::dict_keys_view(rc)),
                        "values" => Ok(Value::dict_values_view(rc)),
                        "items"  => Ok(Value::dict_items_view(rc)),
                        _ => unreachable!(),
                    };
                }
                let dict = regs[obj as usize]
                    .as_dict_mut()
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                pyrust_builtins::dict::call(&method, dict, pos_items)
            }
            3 => {
                if let Some(ValueKind::Tuple(items)) = regs[obj as usize].as_some().map(|v| v.kind()) {
                    pyrust_builtins::tuple::call(&method, items, pos_items)
                } else {
                    Err(PyError::Runtime("internal: expected tuple".to_string()))
                }
            }
            4 => {
                if let Some(v) = regs[obj as usize].as_some() {
                    pyrust_builtins::string::call(&method, v, pos_items)
                } else { unreachable!() }
            }
            5 => {
                let set = regs[obj as usize]
                    .as_set_mut()
                    .ok_or_else(|| PyError::Runtime("internal: expected set".to_string()))?;
                pyrust_builtins::set::call(&method, set, pos_items)
            }
            _ => {
                let obj_val = vm_read(regs, obj, num_locals)?;
                let method_val = self.get_attr(obj_val, &method)?;
                let mut expanded: Vec<ExpandedCallArg> = pos_items
                    .into_iter()
                    .map(|v| ExpandedCallArg { name: None, value: v })
                    .collect();
                for (k, v) in &kw_map {
                    if let PyKey::Str(name) = k {
                        expanded.push(ExpandedCallArg { name: Some(name.clone()), value: v.clone() });
                    }
                }
                let mut buf = std::mem::take(&mut self.call_arg_buf);
                buf.clear();
                buf.extend(expanded);
                let r = self.call_function_expanded(method_val, &buf);
                self.call_arg_buf = buf;
                r
            }
        }
    }
}

#[inline]
fn vm_read(regs: &[Value], reg: crate::bytecode::Reg, num_locals: crate::bytecode::Reg) -> crate::interpreter::Result<Value> {
    let v = &regs[reg as usize];
    if v.is_unset() {
        if reg < num_locals {
            return Err(crate::error::PyError::Named(
                "NameError".to_string(),
                "local variable referenced before assignment".to_string(),
            ));
        } else {
            return Err(crate::error::PyError::Runtime(
                "internal: temp register read before write".to_string(),
            ));
        }
    }
    Ok(v.clone())
}

fn vm_eval_unary(op: UnaryOp, val: Value) -> Result<Value> {
    match op {
        UnaryOp::Neg => match val.kind() {
            ValueKind::Int(v) => Ok(Value::int(-v)),
            ValueKind::Float(v) => Ok(Value::float(-v)),
            _ => Err(PyError::Named("TypeError".to_string(), "bad operand type for unary -".to_string())),
        },
        UnaryOp::Not => Ok(Value::bool_(!val.truthy())),
        UnaryOp::BitNot => match val.kind() {
            ValueKind::Int(v) => Ok(Value::int(!v)),
            ValueKind::Bool(b) => Ok(Value::int(if b { -2 } else { -1 })),
            _ => Err(PyError::Named("TypeError".to_string(),
                "bad operand type for unary ~: use integer".to_string(),
            )),
        },
        UnaryOp::Pos => match val.kind() {
            ValueKind::Int(v) => Ok(Value::int(v)),
            ValueKind::Float(v) => Ok(Value::float(v)),
            ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
            _ => Err(PyError::Named("TypeError".to_string(), "bad operand type for unary +".to_string())),
        },
    }
}

#[cfg(test)]
mod vm_tests {
    use super::*;
    use crate::bytecode::{FnCode, Insn};
    use crate::interpreter::Interpreter;

    fn empty_code(insns: Vec<Insn>) -> FnCode {
        FnCode {
            insns,
            consts: vec![],
            names: vec![],
            num_regs: 0,
            num_iters: 0,
            num_locals: 0,
            fn_protos: vec![],
            cell_vars: vec![],
            is_generator: false,
        }
    }

    #[test]
    fn matchexcept_with_no_active_exception_returns_error() {
        // MatchExcept must error when no exception is active (compiler bug scenario).
        let mut code = empty_code(vec![]);
        code.num_regs = 1;
        code.insns.push(Insn::LoadNone(0));           // type_reg = None (placeholder)
        code.insns.push(Insn::MatchExcept(0, 1));     // no active_exception → error
        code.insns.push(Insn::ReturnNone);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![Value::unset(); 1];
        let result = interp.run_bytecode(&code, &mut regs);
        assert!(result.is_err(), "expected Err, got {:?}", result);
        assert!(
            result.unwrap_err().to_string().contains("no active exception"),
            "error should mention no active exception"
        );
    }

    #[test]
    fn oob_pc_returns_error_not_none() {
        // Jump(100): new_pc = 1 + 100 = 101 > insns.len() (1) → error
        let code = empty_code(vec![Insn::Jump(100)]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let result = interp.run_bytecode(&code, &mut regs);
        assert!(result.is_err(), "expected Err for OOB jump, got {:?}", result);
        assert!(result.unwrap_err().to_string().contains("internal error"));
    }

    #[test]
    fn negative_jump_returns_error() {
        // Jump(-100): new_pc = 1 + (-100) = -99 → underflow error
        let code = empty_code(vec![Insn::Jump(-100)]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let result = interp.run_bytecode(&code, &mut regs);
        assert!(result.is_err(), "expected Err for negative jump, got {:?}", result);
        assert!(result.unwrap_err().to_string().contains("internal error"));
    }

    #[test]
    fn normal_fallthrough_returns_none() {
        let code = empty_code(vec![Insn::ReturnNone]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        assert_eq!(interp.run_bytecode(&code, &mut regs).unwrap(), Value::none());
    }

    #[test]
    fn setup_except_negative_offset_returns_error() {
        // SetupExcept(-100): handler_pc = 1 + (-100) < 0 → error at push time
        let code = empty_code(vec![Insn::SetupExcept(-100), Insn::ReturnNone]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Value> = vec![];
        let result = interp.run_bytecode(&code, &mut regs);
        assert!(result.is_err(), "expected Err for SetupExcept with OOB offset, got {:?}", result);
    }
}
