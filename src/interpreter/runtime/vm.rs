#[derive(Clone)]
enum IterState {
    Materialized(Vec<Value>, usize),
    Range { cur: i64, stop: i64, step: i64 },
    /// Lazy: reads directly from the source register on each ForIter call.
    /// Avoids the O(n) upfront clone that Materialized would require for List/Tuple.
    Indexed { reg: u8, pos: usize },
}

impl Interpreter {
    /// Execute compiled bytecode for a user function.
    ///
    /// `regs` must be pre-sized to `code.num_regs` with parameter slots already filled.
    fn run_bytecode(
        &mut self,
        code: &crate::bytecode::FnCode,
        regs: &mut Vec<Option<Value>>,
    ) -> Result<Value> {
        use crate::bytecode::Insn;
        use std::collections::HashMap;
        let num_locals = code.num_locals;

        let mut iters: Vec<Option<IterState>> = vec![None; code.num_iters as usize];
        let mut exc_handlers: Vec<usize> = Vec::new();
        let mut pc: usize = 0;

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
            let Some(insn) = code.insns.get(pc) else {
                return Ok(Value::None);
            };
            pc += 1;

            match insn {
                // ── Loads ────────────────────────────────────────────────
                Insn::LoadConst(dst, idx) => {
                    regs[*dst as usize] = Some(code.consts[*idx as usize].clone());
                }
                Insn::LoadGlobal(dst, name_idx) => {
                    let name = &code.names[*name_idx as usize];
                    let val = if let Some(v) = vm_try!(self.lookup_name(name)) {
                        v
                    } else {
                        vm_try!(resolve_builtin(name).ok_or_else(|| {
                            PyError::Runtime(format!("name '{}' is not defined", name))
                        }))
                    };
                    regs[*dst as usize] = Some(val);
                }
                Insn::StoreGlobal(name_idx, src) => {
                    let name = code.names[*name_idx as usize].clone();
                    let val = vm_try!(vm_read(regs, *src, num_locals));
                    self.assign_name(name, val);
                }
                Insn::LoadNone(dst) => {
                    regs[*dst as usize] = Some(Value::None);
                }
                Insn::Move(dst, src) => {
                    let v = vm_try!(vm_read(regs, *src, num_locals));
                    regs[*dst as usize] = Some(v);
                }

                // ── Arithmetic / Logic ───────────────────────────────────
                Insn::BinOp(dst, lhs, op, rhs) => {
                    // Fast path: borrow both Int operands to avoid 2× Value clone.
                    if let (Some(Value::Int(a)), Some(Value::Int(b))) =
                        (&regs[*lhs as usize], &regs[*rhs as usize])
                    {
                        match op {
                            BinaryOp::Add => { regs[*dst as usize] = Some(Value::Int(a.wrapping_add(*b))); continue; }
                            BinaryOp::Sub => { regs[*dst as usize] = Some(Value::Int(a.wrapping_sub(*b))); continue; }
                            BinaryOp::Mul => { regs[*dst as usize] = Some(Value::Int(a.wrapping_mul(*b))); continue; }
                            BinaryOp::Eq  => { regs[*dst as usize] = Some(Value::Bool(a == b)); continue; }
                            BinaryOp::Ne  => { regs[*dst as usize] = Some(Value::Bool(a != b)); continue; }
                            BinaryOp::Lt  => { regs[*dst as usize] = Some(Value::Bool(a < b)); continue; }
                            BinaryOp::Le  => { regs[*dst as usize] = Some(Value::Bool(a <= b)); continue; }
                            BinaryOp::Gt  => { regs[*dst as usize] = Some(Value::Bool(a > b)); continue; }
                            BinaryOp::Ge  => { regs[*dst as usize] = Some(Value::Bool(a >= b)); continue; }
                            _ => {}
                        }
                    }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
                    let result = match (&l, op, &r) {
                        (Value::Int(a), BinaryOp::Add, Value::Int(b)) => {
                            Value::Int(a.wrapping_add(*b))
                        }
                        (Value::Int(a), BinaryOp::Sub, Value::Int(b)) => {
                            Value::Int(a.wrapping_sub(*b))
                        }
                        (Value::Int(a), BinaryOp::Mul, Value::Int(b)) => {
                            Value::Int(a.wrapping_mul(*b))
                        }
                        (Value::Int(a), BinaryOp::Eq, Value::Int(b)) => Value::Bool(a == b),
                        (Value::Int(a), BinaryOp::Ne, Value::Int(b)) => Value::Bool(a != b),
                        (Value::Int(a), BinaryOp::Lt, Value::Int(b)) => Value::Bool(a < b),
                        (Value::Int(a), BinaryOp::Le, Value::Int(b)) => Value::Bool(a <= b),
                        (Value::Int(a), BinaryOp::Gt, Value::Int(b)) => Value::Bool(a > b),
                        (Value::Int(a), BinaryOp::Ge, Value::Int(b)) => Value::Bool(a >= b),
                        _ => vm_try!(self.eval_binary(l, *op, r)),
                    };
                    regs[*dst as usize] = Some(result);
                }
                Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
                    let result = match (&l, op, &r) {
                        (Value::Int(a), BinaryOp::Add, Value::Int(b)) => {
                            Value::Int(a.wrapping_add(*b))
                        }
                        (Value::Int(a), BinaryOp::Sub, Value::Int(b)) => {
                            Value::Int(a.wrapping_sub(*b))
                        }
                        (Value::Int(a), BinaryOp::Mul, Value::Int(b)) => {
                            Value::Int(a.wrapping_mul(*b))
                        }
                        (Value::Int(a), BinaryOp::Eq, Value::Int(b)) => Value::Bool(a == b),
                        (Value::Int(a), BinaryOp::Ne, Value::Int(b)) => Value::Bool(a != b),
                        (Value::Int(a), BinaryOp::Lt, Value::Int(b)) => Value::Bool(a < b),
                        (Value::Int(a), BinaryOp::Le, Value::Int(b)) => Value::Bool(a <= b),
                        (Value::Int(a), BinaryOp::Gt, Value::Int(b)) => Value::Bool(a > b),
                        (Value::Int(a), BinaryOp::Ge, Value::Int(b)) => Value::Bool(a >= b),
                        _ => {
                            if *op == BinaryOp::MatMul {
                                if let Some(v) =
                                    vm_try!(self.try_inplace_matmul(l.clone(), r.clone()))
                                {
                                    v
                                } else {
                                    vm_try!(self.eval_binary(l, BinaryOp::MatMul, r))
                                }
                            } else {
                                vm_try!(self.eval_binary(l, *op, r))
                            }
                        }
                    };
                    regs[*dst as usize] = Some(result);
                }
                Insn::BinOpConst(dst, lhs, op, const_idx) => {
                    // Fast path: borrow Int operands to avoid 2× Value clone.
                    if let (Some(Value::Int(a)), Value::Int(b)) = (
                        &regs[*lhs as usize],
                        &code.consts[*const_idx as usize],
                    ) {
                        match op {
                            BinaryOp::Add => { regs[*dst as usize] = Some(Value::Int(a.wrapping_add(*b))); continue; }
                            BinaryOp::Sub => { regs[*dst as usize] = Some(Value::Int(a.wrapping_sub(*b))); continue; }
                            BinaryOp::Mul => { regs[*dst as usize] = Some(Value::Int(a.wrapping_mul(*b))); continue; }
                            BinaryOp::Eq  => { regs[*dst as usize] = Some(Value::Bool(a == b)); continue; }
                            BinaryOp::Ne  => { regs[*dst as usize] = Some(Value::Bool(a != b)); continue; }
                            BinaryOp::Lt  => { regs[*dst as usize] = Some(Value::Bool(a < b)); continue; }
                            BinaryOp::Le  => { regs[*dst as usize] = Some(Value::Bool(a <= b)); continue; }
                            BinaryOp::Gt  => { regs[*dst as usize] = Some(Value::Bool(a > b)); continue; }
                            BinaryOp::Ge  => { regs[*dst as usize] = Some(Value::Bool(a >= b)); continue; }
                            _ => {}
                        }
                    }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = code.consts[*const_idx as usize].clone();
                    let result = match (&l, op, &r) {
                        (Value::Int(a), BinaryOp::Add, Value::Int(b)) => {
                            Value::Int(a.wrapping_add(*b))
                        }
                        (Value::Int(a), BinaryOp::Sub, Value::Int(b)) => {
                            Value::Int(a.wrapping_sub(*b))
                        }
                        (Value::Int(a), BinaryOp::Mul, Value::Int(b)) => {
                            Value::Int(a.wrapping_mul(*b))
                        }
                        (Value::Int(a), BinaryOp::Eq, Value::Int(b)) => Value::Bool(a == b),
                        (Value::Int(a), BinaryOp::Ne, Value::Int(b)) => Value::Bool(a != b),
                        (Value::Int(a), BinaryOp::Lt, Value::Int(b)) => Value::Bool(a < b),
                        (Value::Int(a), BinaryOp::Le, Value::Int(b)) => Value::Bool(a <= b),
                        (Value::Int(a), BinaryOp::Gt, Value::Int(b)) => Value::Bool(a > b),
                        (Value::Int(a), BinaryOp::Ge, Value::Int(b)) => Value::Bool(a >= b),
                        _ => vm_try!(self.eval_binary(l, *op, r)),
                    };
                    regs[*dst as usize] = Some(result);
                }
                Insn::UnaryOp(dst, op, src) => {
                    let val = vm_try!(vm_read(regs, *src, num_locals));
                    let result = vm_try!(vm_eval_unary(*op, val));
                    regs[*dst as usize] = Some(result);
                }

                // ── Attribute / Index ────────────────────────────────────
                Insn::GetAttr(dst, obj, name_idx) => {
                    let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                    let name = &code.names[*name_idx as usize];
                    let result = vm_try!(self.get_attr(obj_val, name));
                    regs[*dst as usize] = Some(result);
                }
                Insn::SetAttr(obj, name_idx, val) => {
                    let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                    let val_val = vm_try!(vm_read(regs, *val, num_locals));
                    let name = &code.names[*name_idx as usize];
                    vm_try!(self.assign_attr(obj_val, name, val_val));
                }
                Insn::DeleteAttr(obj, name_idx) => {
                    let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                    let name = code.names[*name_idx as usize].clone();
                    match obj_val {
                        Value::Instance(inst) => {
                            inst.borrow_mut().attrs.remove(&name);
                        }
                        _ => {
                            vm_try!(Err(PyError::Runtime(
                                "can only delete attributes of class instances".to_string(),
                            )));
                        }
                    }
                }
                Insn::GetItem(dst, obj, idx) => {
                    let idx_val = vm_try!(vm_read(regs, *idx, num_locals));
                    // Slice key: tuple of (lo, hi, step) produced by the compiler.
                    if let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
                        let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                        let result = vm_try!(self.eval_slice(obj_val, lo, hi, st));
                        regs[*dst as usize] = Some(result);
                    } else {
                        // Fast path: read directly from the register without cloning
                        // the entire collection (avoids O(n) clone per GetItem call).
                        let result = match regs[*obj as usize].as_ref() {
                            Some(Value::List(items)) => {
                                let i = vm_try!(normalize_index(&idx_val, items.len()));
                                items[i].clone()
                            }
                            Some(Value::Tuple(items)) => {
                                let i = vm_try!(normalize_index(&idx_val, items.len()));
                                items[i].clone()
                            }
                            Some(Value::Dict(dict)) => {
                                let key = vm_try!(idx_val.to_key().ok_or_else(|| {
                                    PyError::Runtime("unhashable key type".to_string())
                                }));
                                vm_try!(dict
                                    .get(&key)
                                    .cloned()
                                    .ok_or_else(|| PyError::Runtime("key error".to_string())))
                            }
                            _ => {
                                let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                                vm_try!(self.eval_index(obj_val, idx_val))
                            }
                        };
                        regs[*dst as usize] = Some(result);
                    }
                }
                Insn::SetItem(obj, idx, val) => {
                    let idx_val = vm_try!(vm_read(regs, *idx, num_locals));
                    let val_val = vm_try!(vm_read(regs, *val, num_locals));
                    // Slice assignment: tuple key on a list.
                    if let Some((lo, hi, st)) = Self::unpack_slice_key(&idx_val) {
                        let new_items = match val_val {
                            Value::List(v) => v,
                            other => vm_try!(iter_values(other).map_err(|_| {
                                PyError::Runtime("slice assignment requires iterable".to_string())
                            })),
                        };
                        match regs[*obj as usize].as_mut() {
                            Some(Value::List(items)) => {
                                vm_try!(Self::slice_setitem(
                                    items,
                                    lo.as_ref(),
                                    hi.as_ref(),
                                    st.as_ref(),
                                    new_items,
                                ));
                            }
                            _ => {
                                vm_try!(Err(PyError::Runtime(
                                    "object does not support slice assignment".to_string(),
                                )));
                            }
                        }
                    } else {
                        match regs[*obj as usize].as_mut() {
                            Some(Value::List(items)) => {
                                let i = vm_try!(normalize_index(&idx_val, items.len()));
                                items[i] = val_val;
                            }
                            Some(Value::Dict(dict)) => {
                                let key = vm_try!(idx_val.to_key().ok_or_else(|| {
                                    PyError::Runtime("unhashable type".to_string())
                                }));
                                dict.insert(key, val_val);
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
                        match regs[*obj as usize].as_mut() {
                            Some(Value::List(items)) => {
                                vm_try!(Self::slice_delitem(
                                    items,
                                    lo.as_ref(),
                                    hi.as_ref(),
                                    st.as_ref(),
                                ));
                            }
                            _ => {
                                vm_try!(Err(PyError::Runtime(
                                    "object does not support slice deletion".to_string(),
                                )));
                            }
                        }
                    } else {
                        match regs[*obj as usize].as_mut() {
                            Some(Value::List(items)) => {
                                let i = vm_try!(normalize_index(&idx_val, items.len()));
                                items.remove(i);
                            }
                            Some(Value::Dict(dict)) => {
                                let key = vm_try!(idx_val.to_key().ok_or_else(|| {
                                    PyError::Runtime("unhashable type".to_string())
                                }));
                                dict.remove(&key);
                            }
                            _ => {
                                vm_try!(Err(PyError::Runtime(
                                    "object does not support item deletion".to_string(),
                                )));
                            }
                        }
                    }
                }
                Insn::DeleteName(name_idx) => {
                    let name = code.names[*name_idx as usize].clone();
                    self.env.borrow_mut().values.remove(&name);
                }

                // ── Control flow ─────────────────────────────────────────
                Insn::Jump(offset) => {
                    pc = (pc as i32 + offset) as usize;
                }
                Insn::JumpIfFalse(cond, offset) => {
                    if !vm_try!(vm_read(regs, *cond, num_locals)).truthy() {
                        pc = (pc as i32 + offset) as usize;
                    }
                }
                Insn::JumpIfTrue(cond, offset) => {
                    if vm_try!(vm_read(regs, *cond, num_locals)).truthy() {
                        pc = (pc as i32 + offset) as usize;
                    }
                }
                Insn::CmpJumpIfFalse(lhs, op, rhs, offset) => {
                    if let (Some(Value::Int(a)), Some(Value::Int(b))) =
                        (&regs[*lhs as usize], &regs[*rhs as usize])
                    {
                        match op {
                            BinaryOp::Eq => { if !(a == b) { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Ne => { if !(a != b) { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Lt => { if !(a < b)  { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Le => { if !(a <= b) { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Gt => { if !(a > b)  { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Ge => { if !(a >= b) { pc = (pc as i32 + offset) as usize; } continue; }
                            _ => {}
                        }
                    }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
                    if !vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = (pc as i32 + offset) as usize; }
                }
                Insn::CmpJumpIfTrue(lhs, op, rhs, offset) => {
                    if let (Some(Value::Int(a)), Some(Value::Int(b))) =
                        (&regs[*lhs as usize], &regs[*rhs as usize])
                    {
                        match op {
                            BinaryOp::Eq => { if a == b { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Ne => { if a != b { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Lt => { if a < b  { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Le => { if a <= b { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Gt => { if a > b  { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Ge => { if a >= b { pc = (pc as i32 + offset) as usize; } continue; }
                            _ => {}
                        }
                    }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
                    if vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = (pc as i32 + offset) as usize; }
                }
                Insn::CmpJumpIfFalseConst(lhs, op, const_idx, offset) => {
                    if let (Some(Value::Int(a)), Value::Int(b)) =
                        (&regs[*lhs as usize], &code.consts[*const_idx as usize])
                    {
                        match op {
                            BinaryOp::Eq => { if !(a == b) { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Ne => { if !(a != b) { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Lt => { if !(a < b)  { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Le => { if !(a <= b) { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Gt => { if !(a > b)  { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Ge => { if !(a >= b) { pc = (pc as i32 + offset) as usize; } continue; }
                            _ => {}
                        }
                    }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = code.consts[*const_idx as usize].clone();
                    if !vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = (pc as i32 + offset) as usize; }
                }
                Insn::CmpJumpIfTrueConst(lhs, op, const_idx, offset) => {
                    if let (Some(Value::Int(a)), Value::Int(b)) =
                        (&regs[*lhs as usize], &code.consts[*const_idx as usize])
                    {
                        match op {
                            BinaryOp::Eq => { if a == b { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Ne => { if a != b { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Lt => { if a < b  { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Le => { if a <= b { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Gt => { if a > b  { pc = (pc as i32 + offset) as usize; } continue; }
                            BinaryOp::Ge => { if a >= b { pc = (pc as i32 + offset) as usize; } continue; }
                            _ => {}
                        }
                    }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = code.consts[*const_idx as usize].clone();
                    if vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = (pc as i32 + offset) as usize; }
                }

                // ── Exception handling ───────────────────────────────────
                Insn::SetupExcept(offset) => {
                    exc_handlers.push((pc as i32 + offset) as usize);
                }
                Insn::PopExcept => {
                    exc_handlers.pop();
                }
                Insn::LoadExc(dst) => {
                    let exc = vm_try!(self.active_exception.clone().ok_or_else(|| {
                        PyError::Runtime("no active exception".to_string())
                    }));
                    regs[*dst as usize] = Some(exc);
                }
                Insn::MatchExcept(type_reg, offset) => {
                    let type_val = vm_try!(vm_read(regs, *type_reg, num_locals));
                    let exc = self.active_exception.clone().unwrap_or(Value::None);
                    if !vm_try!(self.exception_matches(&exc, &type_val)) {
                        pc = (pc as i32 + offset) as usize;
                    }
                }
                Insn::EndExcept => {
                    self.active_exception = None;
                }
                Insn::RaiseAssert(msg_reg) => {
                    let msg = vm_try!(vm_read(regs, *msg_reg, num_locals));
                    let msg_str = match msg {
                        Value::None => String::new(),
                        other => other.to_py_str(),
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
                    if let Value::Instance(ref inst) = exc {
                        inst.borrow_mut().attrs.insert("__cause__".to_string(), cause);
                        inst.borrow_mut().attrs.insert("__suppress_context__".to_string(), Value::Bool(true));
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
                    // Reuse the interpreter-level buffer to avoid a per-call heap
                    // allocation in the common (non-recursive) case.
                    let mut buf = std::mem::take(&mut self.call_arg_buf);
                    buf.clear();
                    for i in 0..*argc as usize {
                        buf.push(ExpandedCallArg {
                            name: None,
                            value: vm_try!(vm_read(
                                regs,
                                *func_reg + 1 + i as u8,
                                num_locals
                            )),
                        });
                    }
                    let call_result = self.call_function_expanded(func_val, &buf);
                    self.call_arg_buf = buf;
                    regs[*func_reg as usize] = Some(vm_try!(call_result));
                }

                // ── Returns ──────────────────────────────────────────────
                Insn::Return(src) => {
                    return Ok(vm_try!(vm_read(regs, *src, num_locals)));
                }
                Insn::ReturnNone => {
                    return Ok(Value::None);
                }

                // ── Collection builders ──────────────────────────────────
                Insn::BuildList(dst, base, n) => {
                    let mut items = Vec::with_capacity(*n as usize);
                    for i in 0..*n as usize {
                        items.push(vm_try!(vm_read(regs, *base + i as u8, num_locals)));
                    }
                    regs[*dst as usize] = Some(Value::List(items));
                }
                Insn::BuildTuple(dst, base, n) => {
                    let mut items = Vec::with_capacity(*n as usize);
                    for i in 0..*n as usize {
                        items.push(vm_try!(vm_read(regs, *base + i as u8, num_locals)));
                    }
                    regs[*dst as usize] = Some(Value::Tuple(items));
                }
                Insn::BuildDict(dst, base, n) => {
                    let mut dict = indexmap::IndexMap::new();
                    for i in 0..*n as usize {
                        let k_val =
                            vm_try!(vm_read(regs, *base + (i * 2) as u8, num_locals));
                        let v_val =
                            vm_try!(vm_read(regs, *base + (i * 2 + 1) as u8, num_locals));
                        let key = vm_try!(k_val.to_key().ok_or_else(|| {
                            PyError::Runtime("unhashable type in dict key".to_string())
                        }));
                        dict.insert(key, v_val);
                    }
                    regs[*dst as usize] = Some(Value::Dict(dict));
                }
                Insn::ListAppend(list_reg, val_reg) => {
                    let val = vm_try!(vm_read(regs, *val_reg, num_locals));
                    match regs[*list_reg as usize].as_mut() {
                        Some(Value::List(items)) => items.push(val),
                        _ => {
                            vm_try!(Err::<(), _>(PyError::Runtime(
                                "ListAppend on non-list".to_string(),
                            )));
                        }
                    }
                }
                Insn::ListExtend(list_reg, src_reg) => {
                    let src_val = vm_try!(vm_read(regs, *src_reg, num_locals));
                    let items_to_add = vm_try!(iter_values(src_val));
                    match regs[*list_reg as usize].as_mut() {
                        Some(Value::List(items)) => items.extend(items_to_add),
                        _ => {
                            vm_try!(Err::<(), _>(PyError::Runtime(
                                "ListExtend on non-list".to_string(),
                            )));
                        }
                    }
                }
                Insn::DictUpdate(dict_reg, src_reg) => {
                    let src_val = vm_try!(vm_read(regs, *src_reg, num_locals));
                    match src_val {
                        Value::Dict(src_dict) => {
                            let pairs: Vec<(PyKey, Value)> =
                                src_dict.into_iter().collect();
                            match regs[*dict_reg as usize].as_mut() {
                                Some(Value::Dict(dict)) => {
                                    for (k, v) in pairs {
                                        dict.insert(k, v);
                                    }
                                }
                                _ => {
                                    vm_try!(Err::<(), _>(PyError::Runtime(
                                        "DictUpdate on non-dict".to_string(),
                                    )));
                                }
                            }
                        }
                        _ => {
                            vm_try!(Err::<(), _>(PyError::Runtime(
                                "DictUpdate requires a dict argument".to_string(),
                            )));
                        }
                    }
                }

                // ── Unpack ───────────────────────────────────────────────
                Insn::Unpack(base, src, n) => {
                    let src_val = vm_try!(vm_read(regs, *src, num_locals));
                    let items = vm_try!(iter_values(src_val));
                    if items.len() != *n as usize {
                        vm_try!(Err::<(), _>(PyError::Runtime(format!(
                            "not enough values to unpack (expected {}, got {})",
                            n,
                            items.len()
                        ))));
                    }
                    for (i, v) in items.into_iter().enumerate() {
                        regs[*base as usize + i] = Some(v);
                    }
                }

                // ── Iterator ─────────────────────────────────────────────
                Insn::GetIter(slot, src) => {
                    // Range: lazy counter — no Vec needed.
                    // List/Tuple in a LOCAL register: lazy index — avoids the O(n)
                    //   upfront clone; the local slot is stable for the function lifetime.
                    // Temp register or other type: materialise immediately, because
                    //   a temp reg is freed and may be overwritten by the loop body.
                    let state = match regs[*src as usize].as_ref() {
                        Some(Value::List(_)) | Some(Value::Tuple(_))
                            if *src < num_locals =>
                        {
                            IterState::Indexed { reg: *src, pos: 0 }
                        }
                        _ => {
                            let src_val = vm_try!(vm_read(regs, *src, num_locals));
                            match src_val {
                                Value::Range { start, stop, step } => {
                                    IterState::Range { cur: start, stop, step }
                                }
                                other => {
                                    IterState::Materialized(vm_try!(iter_values(other)), 0)
                                }
                            }
                        }
                    };
                    iters[*slot as usize] = Some(state);
                }
                Insn::ForIter(dst, slot, offset) => {
                    match iters[*slot as usize].as_mut() {
                        Some(IterState::Materialized(items, pos)) => {
                            if *pos < items.len() {
                                let v = items[*pos].clone();
                                *pos += 1;
                                regs[*dst as usize] = Some(v);
                            } else {
                                pc = (pc as i32 + offset) as usize;
                            }
                        }
                        Some(IterState::Range { cur, stop, step }) => {
                            let exhausted =
                                if *step > 0 { *cur >= *stop } else { *cur <= *stop };
                            if exhausted {
                                pc = (pc as i32 + offset) as usize;
                            } else {
                                let v = Value::Int(*cur);
                                *cur += *step;
                                regs[*dst as usize] = Some(v);
                            }
                        }
                        Some(IterState::Indexed { reg, pos }) => {
                            let src = *reg as usize;
                            let cur_pos = *pos;
                            let v_opt = match regs[src].as_ref() {
                                Some(Value::List(items)) if cur_pos < items.len() => {
                                    Some(items[cur_pos].clone())
                                }
                                Some(Value::Tuple(items)) if cur_pos < items.len() => {
                                    Some(items[cur_pos].clone())
                                }
                                _ => None,
                            };
                            if let Some(v) = v_opt {
                                *pos += 1;
                                regs[*dst as usize] = Some(v);
                            } else {
                                pc = (pc as i32 + offset) as usize;
                            }
                        }
                        None => {
                            pc = (pc as i32 + offset) as usize;
                        }
                    }
                }
                Insn::ForCountReg(var, cmp_op, stop_reg, step_idx, offset) => {
                    let cur = match vm_try!(vm_read(regs, *var, num_locals)) {
                        Value::Int(i) => i,
                        _ => vm_try!(Err(crate::error::PyError::Runtime(
                            "for-range: non-integer counter".into(),
                        ))),
                    };
                    let step = match &code.consts[*step_idx as usize] {
                        Value::Int(s) => *s,
                        _ => unreachable!("ForCountReg step must be Int"),
                    };
                    let next = cur.wrapping_add(step);
                    let stop = match vm_try!(vm_read(regs, *stop_reg, num_locals)) {
                        Value::Int(s) => s,
                        _ => vm_try!(Err(crate::error::PyError::Runtime(
                            "for-range: non-integer stop".into(),
                        ))),
                    };
                    let cont = match cmp_op {
                        BinaryOp::Lt => next < stop,
                        BinaryOp::Gt => next > stop,
                        _ => unreachable!("ForCountReg uses Lt or Gt only"),
                    };
                    if cont {
                        regs[*var as usize] = Some(Value::Int(next));
                    } else {
                        pc = (pc as i32 + offset) as usize;
                    }
                }
                Insn::ForCountConst(var, cmp_op, stop_idx, step_idx, offset) => {
                    let cur = match vm_try!(vm_read(regs, *var, num_locals)) {
                        Value::Int(i) => i,
                        _ => vm_try!(Err(crate::error::PyError::Runtime(
                            "for-range: non-integer counter".into(),
                        ))),
                    };
                    let step = match &code.consts[*step_idx as usize] {
                        Value::Int(s) => *s,
                        _ => unreachable!("ForCountConst step must be Int"),
                    };
                    let next = cur.wrapping_add(step);
                    let stop = match &code.consts[*stop_idx as usize] {
                        Value::Int(s) => *s,
                        _ => unreachable!("ForCountConst stop must be Int"),
                    };
                    let cont = match cmp_op {
                        BinaryOp::Lt => next < stop,
                        BinaryOp::Gt => next > stop,
                        _ => unreachable!("ForCountConst uses Lt or Gt only"),
                    };
                    if cont {
                        regs[*var as usize] = Some(Value::Int(next));
                    } else {
                        pc = (pc as i32 + offset) as usize;
                    }
                }
                Insn::CheckLocal(reg, name_idx) => {
                    if regs[*reg as usize].is_none() {
                        let name = &code.names[*name_idx as usize];
                        vm_try!(Err::<(), _>(crate::error::PyError::Runtime(format!(
                            "cannot access local variable '{}' where it is not associated with a value",
                            name
                        ))));
                    }
                }

                // ── Function / Class creation ────────────────────────────
                Insn::MakeFunction(dst, proto_idx, defs_base, defs_n) => {
                    let proto = &code.fn_protos[*proto_idx as usize];
                    let proto_code = Rc::clone(&proto.code);
                    let proto_name = proto.name.clone();
                    let proto_local_index = Rc::clone(&proto.local_index);
                    let proto_global_names = Rc::clone(&proto.global_names);
                    let proto_nonlocal_names = Rc::clone(&proto.nonlocal_names);
                    let param_names = proto.param_names.clone();
                    let param_has_default = proto.param_has_default.clone();
                    let param_is_args = proto.param_is_args.clone();
                    let param_is_kwargs = proto.param_is_kwargs.clone();
                    let is_pure = proto.is_pure;
                    let def_bound_mask = proto.def_bound_mask;

                    let mut params = Vec::new();
                    let mut def_slot = 0u8;
                    for i in 0..param_names.len() {
                        let default = if param_has_default[i] {
                            let v =
                                vm_try!(vm_read(regs, *defs_base + def_slot, num_locals));
                            def_slot += 1;
                            Some(v)
                        } else {
                            None
                        };
                        params.push(UserFunctionParam {
                            name: param_names[i].clone(),
                            default,
                            is_args: param_is_args[i],
                            is_kwargs: param_is_kwargs[i],
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
                        name: proto_name,
                        params,
                        body: vec![],
                        local_names: Rc::new(proto_local_index.keys().cloned().collect()),
                        local_index: proto_local_index,
                        global_names: proto_global_names,
                        nonlocal_names: proto_nonlocal_names,
                        env: Rc::clone(&self.env),
                        is_pure,
                        def_bound_mask,
                        precompiled_code: Some(proto_code),
                    });
                    regs[*dst as usize] = Some(Value::Function(func));
                }
                Insn::MakeClass(dst, proto_idx, bases_base, bases_n, name_idx) => {
                    let class_name = code.names[*name_idx as usize].clone();
                    let (class_code, local_index) = {
                        let proto = &code.fn_protos[*proto_idx as usize];
                        (Rc::clone(&proto.code), Rc::clone(&proto.local_index))
                    };
                    let num_class_regs = class_code.num_regs as usize;
                    let mut class_regs: Vec<Option<Value>> = vec![None; num_class_regs];
                    vm_try!(self.run_bytecode(&class_code, &mut class_regs));
                    let mut attrs = HashMap::new();
                    for (attr_name, &slot) in local_index.iter() {
                        if let Some(val) = class_regs.get(slot).and_then(|v| v.clone()) {
                            attrs.insert(attr_name.clone(), val);
                        }
                    }
                    let base = if *bases_n > 0 {
                        let base_val = vm_try!(vm_read(regs, *bases_base, num_locals));
                        match base_val {
                            Value::Class(c) => Some(c),
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
                    regs[*dst as usize] = Some(Value::Class(class));
                }

                // ── Import ───────────────────────────────────────────────
                Insn::ImportModule(dst, name_idx) => {
                    let name = code.names[*name_idx as usize].clone();
                    let module = vm_try!(self.load_module(&name));
                    regs[*dst as usize] = Some(module);
                }

                // ── REPL output ──────────────────────────────────────────
                Insn::PrintExpr(src) => {
                    let val = vm_try!(vm_read(regs, *src, num_locals));
                    if !matches!(val, Value::None) {
                        println!("{}", val.repr());
                    }
                }
            }
        }
    }
}

fn vm_read(regs: &[Option<Value>], reg: u8, num_locals: u8) -> crate::interpreter::Result<Value> {
    match regs[reg as usize].clone() {
        Some(v) => Ok(v),
        None => {
            if reg < num_locals {
                Err(crate::error::PyError::Runtime(
                    "local variable referenced before assignment".to_string(),
                ))
            } else {
                Err(crate::error::PyError::Runtime(
                    "internal: temp register read before write".to_string(),
                ))
            }
        }
    }
}

fn vm_eval_unary(op: UnaryOp, val: Value) -> Result<Value> {
    match op {
        UnaryOp::Neg => match val {
            Value::Int(v) => Ok(Value::Int(-v)),
            Value::Float(v) => Ok(Value::Float(-v)),
            _ => Err(PyError::Runtime("bad operand type for unary -".to_string())),
        },
        UnaryOp::Not => Ok(Value::Bool(!val.truthy())),
        UnaryOp::BitNot => match val {
            Value::Int(v) => Ok(Value::Int(!v)),
            Value::Bool(b) => Ok(Value::Int(if b { -2 } else { -1 })),
            _ => Err(PyError::Runtime(
                "bad operand type for unary ~: use integer".to_string(),
            )),
        },
        UnaryOp::Pos => match val {
            Value::Int(v) => Ok(Value::Int(v)),
            Value::Float(v) => Ok(Value::Float(v)),
            Value::Bool(b) => Ok(Value::Int(if b { 1 } else { 0 })),
            _ => Err(PyError::Runtime("bad operand type for unary +".to_string())),
        },
    }
}
