#[derive(Clone)]
enum IterState {
    Materialized(Vec<Value>, usize),
    Range { cur: i64, stop: i64, step: i64 },
    /// Lazy: reads directly from the source register on each ForIter call.
    /// Avoids the O(n) upfront clone that Materialized would require for List/Tuple.
    Indexed { reg: crate::bytecode::Reg, pos: usize },
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
    regs: &'a mut Vec<Option<Value>>,
    reg: u32,
    op: &str,
) -> Result<&'a mut Vec<Value>> {
    regs[reg as usize]
        .as_mut()
        .and_then(|v| v.as_list_mut())
        .ok_or_else(|| PyError::Runtime(format!("{op} on non-list")))
}

fn expect_dict_mut<'a>(
    regs: &'a mut Vec<Option<Value>>,
    reg: u32,
    op: &str,
) -> Result<&'a mut indexmap::IndexMap<PyKey, Value>> {
    regs[reg as usize]
        .as_mut()
        .and_then(|v| v.as_dict_mut())
        .ok_or_else(|| PyError::Runtime(format!("{op} on non-dict")))
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
                        regs[*dst as usize] = Some(Value::int(n));
                    } else {
                        regs[*dst as usize] = Some(cv.clone());
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
                    regs[*dst as usize] = Some(val);
                }
                Insn::StoreGlobal(name_idx, src) => {
                    let name = pool_get!(code.names, *name_idx, "name").clone();
                    let val = vm_try!(vm_read(regs, *src, num_locals));
                    self.assign_name(name, val);
                }
                Insn::LoadNone(dst) => {
                    regs[*dst as usize] = Some(Value::none());
                }
                Insn::Move(dst, src) => {
                    if let Some(v) = &regs[*src as usize] {
                        if let ValueKind::Int(n) = v.kind() {
                            regs[*dst as usize] = Some(Value::int(n));
                            continue;
                        }
                    }
                    let v = vm_try!(vm_read(regs, *src, num_locals));
                    regs[*dst as usize] = Some(v);
                }

                // ── Arithmetic / Logic ───────────────────────────────────
                Insn::BinOp(dst, lhs, op, rhs) => {
                    if let (Some(lv), Some(rv)) = (&regs[*lhs as usize], &regs[*rhs as usize]) {
                        if let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), rv.kind()) {
                            if let Some(result) = int_int_fast(a, b, *op) {
                                regs[*dst as usize] = Some(result);
                                continue;
                            }
                        }
                    }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
                    regs[*dst as usize] = Some(vm_try!(self.eval_binary(l, *op, r)));
                }
                Insn::BinOpInPlace(dst, lhs, op, rhs) => {
                    if let (Some(lv), Some(rv)) = (&regs[*lhs as usize], &regs[*rhs as usize]) {
                        if let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), rv.kind()) {
                            if let Some(result) = int_int_fast(a, b, *op) {
                                regs[*dst as usize] = Some(result);
                                continue;
                            }
                        }
                    }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
                    let result = if *op == BinaryOp::MatMul {
                        if let Some(v) = vm_try!(self.try_inplace_matmul(l.clone(), r.clone())) { v }
                        else { vm_try!(self.eval_binary(l, BinaryOp::MatMul, r)) }
                    } else {
                        vm_try!(self.eval_binary(l, *op, r))
                    };
                    regs[*dst as usize] = Some(result);
                }
                Insn::BinOpConst(dst, lhs, op, const_idx) => {
                    let cv = pool_get!(code.consts, *const_idx, "const");
                    if let Some(lv) = &regs[*lhs as usize] {
                        if let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), cv.kind()) {
                            if let Some(result) = int_int_fast(a, b, *op) {
                                regs[*dst as usize] = Some(result);
                                continue;
                            }
                        }
                    }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = cv.clone();
                    let result = vm_try!(self.eval_binary(l, *op, r));
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
                    let name = pool_get!(code.names, *name_idx, "name");
                    let result = vm_try!(self.get_attr(obj_val, name));
                    regs[*dst as usize] = Some(result);
                }
                Insn::SetAttr(obj, name_idx, val) => {
                    let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                    let val_val = vm_try!(vm_read(regs, *val, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name");
                    vm_try!(self.assign_attr(obj_val, name, val_val));
                }
                Insn::DeleteAttr(obj, name_idx) => {
                    let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                    let name = pool_get!(code.names, *name_idx, "name").clone();
                    match obj_val.kind() {
                        ValueKind::PyInstance(inst) => {
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
                    // Fast path: List/Tuple indexed by Int — borrow idx, avoid clone.
                    let fast_int_idx = if let Some(iv) = &regs[*idx as usize] {
                        if let ValueKind::Int(raw_i) = iv.kind() { Some(raw_i) } else { None }
                    } else { None };

                    if let Some(raw_i) = fast_int_idx {
                        let mut handled = false;
                        if let Some(ov) = &regs[*obj as usize] {
                            match ov.kind() {
                                ValueKind::List(items) => {
                                    let len = items.len() as i64;
                                    let j = if raw_i < 0 { raw_i + len } else { raw_i };
                                    if j >= 0 && (j as usize) < items.len() {
                                        regs[*dst as usize] = Some(items[j as usize].clone());
                                    } else {
                                        vm_try!(Err(PyError::Runtime("index out of range".into())));
                                    }
                                    handled = true;
                                }
                                ValueKind::Tuple(items) => {
                                    let len = items.len() as i64;
                                    let j = if raw_i < 0 { raw_i + len } else { raw_i };
                                    if j >= 0 && (j as usize) < items.len() {
                                        regs[*dst as usize] = Some(items[j as usize].clone());
                                    } else {
                                        vm_try!(Err(PyError::Runtime("index out of range".into())));
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
                        regs[*dst as usize] = Some(result);
                    } else {
                        // Fast path: read directly from the register without cloning
                        // the entire collection (avoids O(n) clone per GetItem call).
                        let result = if let Some(ov) = &regs[*obj as usize] {
                            match ov.kind() {
                                ValueKind::List(items) => {
                                    let i = vm_try!(normalize_index(&idx_val, items.len()));
                                    Some(items[i].clone())
                                }
                                ValueKind::Tuple(items) => {
                                    let i = vm_try!(normalize_index(&idx_val, items.len()));
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
                            regs[*dst as usize] = Some(r);
                        } else {
                            let obj_val = vm_try!(vm_read(regs, *obj, num_locals));
                            let r = vm_try!(self.eval_index(obj_val, idx_val));
                            regs[*dst as usize] = Some(r);
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
                        let list_mut = if let Some(ov) = regs[*obj as usize].as_mut() {
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
                        let target_kind = regs[*obj as usize].as_ref().map(|v| match v.kind() {
                            ValueKind::List(_) => 1u8,
                            ValueKind::Dict(_) => 2u8,
                            _ => 0u8,
                        }).unwrap_or(0);
                        match target_kind {
                            1 => {
                                let items = vm_try!(expect_list_mut(regs, *obj, "SetItem"));
                                let i = vm_try!(normalize_index(&idx_val, items.len()));
                                items[i] = val_val;
                            }
                            2 => {
                                let dict = vm_try!(expect_dict_mut(regs, *obj, "SetItem"));
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
                        let list_mut = if let Some(ov) = regs[*obj as usize].as_mut() {
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
                        if let Some(ov) = regs[*obj as usize].as_mut() {
                            if let Some(items) = ov.as_list_mut() {
                                let i = vm_try!(normalize_index(&idx_val, items.len()));
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
                            vm_try!(Err(PyError::Runtime(
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
                    regs[*reg as usize] = None;
                }

                // ── Control flow ─────────────────────────────────────────
                Insn::Jump(offset) => {
                    pc = jump_pc!(*offset);
                }
                Insn::JumpIfFalse(cond, offset) => {
                    let fast = if let Some(cv) = &regs[*cond as usize] {
                        match cv.kind() {
                            ValueKind::Int(n)  => { if n == 0 { pc = jump_pc!(*offset); } true }
                            ValueKind::Bool(b) => { if !b    { pc = jump_pc!(*offset); } true }
                            _ => false,
                        }
                    } else { false };
                    if fast { continue; }
                    if !vm_try!(vm_read(regs, *cond, num_locals)).truthy() {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::JumpIfTrue(cond, offset) => {
                    let fast = if let Some(cv) = &regs[*cond as usize] {
                        match cv.kind() {
                            ValueKind::Int(n)  => { if n != 0 { pc = jump_pc!(*offset); } true }
                            ValueKind::Bool(b) => { if b      { pc = jump_pc!(*offset); } true }
                            _ => false,
                        }
                    } else { false };
                    if fast { continue; }
                    if vm_try!(vm_read(regs, *cond, num_locals)).truthy() {
                        pc = jump_pc!(*offset);
                    }
                }
                Insn::CmpJumpIfFalse(lhs, op, rhs, offset) => {
                    if let (Some(lv), Some(rv)) = (&regs[*lhs as usize], &regs[*rhs as usize]) {
                        if let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), rv.kind()) {
                            if let Some(cond) = int_cmp(a, b, *op) {
                                if !cond { pc = jump_pc!(*offset); }
                                continue;
                            }
                        }
                    }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
                    if !vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = jump_pc!(*offset); }
                }
                Insn::CmpJumpIfTrue(lhs, op, rhs, offset) => {
                    if let (Some(lv), Some(rv)) = (&regs[*lhs as usize], &regs[*rhs as usize]) {
                        if let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), rv.kind()) {
                            if let Some(cond) = int_cmp(a, b, *op) {
                                if cond { pc = jump_pc!(*offset); }
                                continue;
                            }
                        }
                    }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = vm_try!(vm_read(regs, *rhs, num_locals));
                    if vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = jump_pc!(*offset); }
                }
                Insn::CmpJumpIfFalseConst(lhs, op, const_idx, offset) => {
                    let cv = pool_get!(code.consts, *const_idx, "const");
                    if let Some(lv) = &regs[*lhs as usize] {
                        if let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), cv.kind()) {
                            if let Some(cond) = int_cmp(a, b, *op) {
                                if !cond { pc = jump_pc!(*offset); }
                                continue;
                            }
                        }
                    }
                    let l = vm_try!(vm_read(regs, *lhs, num_locals));
                    let r = cv.clone();
                    if !vm_try!(self.eval_binary(l, *op, r)).truthy() { pc = jump_pc!(*offset); }
                }
                Insn::CmpJumpIfTrueConst(lhs, op, const_idx, offset) => {
                    let cv = pool_get!(code.consts, *const_idx, "const");
                    if let Some(lv) = &regs[*lhs as usize] {
                        if let (ValueKind::Int(a), ValueKind::Int(b)) = (lv.kind(), cv.kind()) {
                            if let Some(cond) = int_cmp(a, b, *op) {
                                if cond { pc = jump_pc!(*offset); }
                                continue;
                            }
                        }
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
                    regs[*dst as usize] = Some(exc);
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
                    if let ValueKind::PyInstance(ref inst) = exc.kind() {
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
                    if *argc == 1 {
                        if let ValueKind::BuiltinFunction("id") = func_val.kind() {
                            let maybe_id: Option<i64> = regs
                                .get((*func_reg + 1) as usize)
                                .and_then(|o| o.as_ref())
                                .and_then(|v| v.value_id());
                            if let Some(id_val) = maybe_id {
                                regs[*func_reg as usize] = Some(Value::int(id_val));
                                continue 'vm;
                            }
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
                    regs[*func_reg as usize] = Some(vm_try!(call_result));
                }

                Insn::CallMemo(func_reg, argc) => {
                    // Cache-first path for known-pure callees.
                    let is_pure_fn = if let Some(fv) = &regs[*func_reg as usize] {
                        if let ValueKind::UserFunction(func) = fv.kind() {
                            func.is_pure
                        } else { false }
                    } else { false };

                    if is_pure_fn {
                        if let Some(fv) = &regs[*func_reg as usize] {
                            if let ValueKind::UserFunction(func) = fv.kind() {
                                let fn_id = func.id;
                                let mut key = std::mem::take(&mut self.key_scratch);
                                key.clear();
                                let mut all_hashable = true;
                                for i in 0..*argc as usize {
                                    match regs[*func_reg as usize + 1 + i]
                                        .as_ref()
                                        .and_then(|v| v.to_key())
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
                                        regs[*func_reg as usize] = Some(cached);
                                        continue;
                                    }
                                } else {
                                    self.key_scratch = key;
                                }
                            }
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
                    regs[*func_reg as usize] = Some(vm_try!(call_result));
                }

                Insn::CallMethod { dst, obj, name_idx, args_base, nargs } => {
                    let r = self.exec_call_method(regs, num_locals, *dst, *obj, *name_idx, *args_base, *nargs, code);
                    regs[*dst as usize] = Some(vm_try!(r));
                }

                Insn::CallMethodExpanded { dst, obj, name_idx, pos_list, kw_dict } => {
                    let r = self.exec_call_method_expanded(regs, num_locals, *dst, *obj, *name_idx, *pos_list, *kw_dict, code);
                    regs[*dst as usize] = Some(vm_try!(r));
                }

                // ── Returns ──────────────────────────────────────────────
                Insn::Return(src) => {
                    return Ok(vm_try!(vm_read(regs, *src, num_locals)));
                }
                Insn::ReturnNone => {
                    return Ok(Value::none());
                }

                // ── Collection builders ──────────────────────────────────
                Insn::BuildList(dst, base, n) => {
                    let mut items: Vec<Value> = Vec::with_capacity(*n as usize);
                    for i in 0..crate::bytecode::Reg::from(*n) {
                        items.push(vm_try!(vm_read(regs, *base + i, num_locals)));
                    }
                    regs[*dst as usize] = Some(Value::list(items));
                }
                Insn::BuildTuple(dst, base, n) => {
                    let mut items = Vec::with_capacity(*n as usize);
                    for i in 0..crate::bytecode::Reg::from(*n) {
                        items.push(vm_try!(vm_read(regs, *base + i, num_locals)));
                    }
                    regs[*dst as usize] = Some(Value::tuple(items));
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
                    regs[*dst as usize] = Some(Value::dict(dict));
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
                        regs[dst] = Some(v);
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
                        if let Some(v) = &regs[*src as usize] {
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
                            _ => {
                                IterState::Materialized(vm_try!(iter_values(src_val)), 0)
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
                                regs[*dst as usize] = Some(v);
                            }
                        }
                        Some(IterState::Indexed { reg, pos }) => {
                            let src = *reg as usize;
                            let cur_pos = *pos;
                            let v_opt: Option<Value> = if let Some(rv) = &regs[src] {
                                match rv.kind() {
                                    ValueKind::List(items) if cur_pos < items.len() => {
                                        Some(items[cur_pos].clone())
                                    }
                                    ValueKind::Tuple(items) if cur_pos < items.len() => {
                                        Some(items[cur_pos].clone())
                                    }
                                    _ => None,
                                }
                            } else { None };
                            if let Some(v) = v_opt {
                                *pos += 1;
                                regs[*dst as usize] = Some(v);
                            } else {
                                pc = jump_pc!(*offset);
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
                    let fast = if let (Some(vv), Some(sv)) = (&regs[*var as usize], &regs[*stop_reg as usize]) {
                        if let (ValueKind::Int(cur), ValueKind::Int(stop)) = (vv.kind(), sv.kind()) {
                            let next = cur.wrapping_add(step);
                            let cont = match cmp_op {
                                BinaryOp::Lt => next < stop,
                                BinaryOp::Gt => next > stop,
                                _ => unreachable!("ForCountReg uses Lt or Gt only"),
                            };
                            if cont { regs[*var as usize] = Some(Value::int(next)); }
                            else { pc = jump_pc!(*offset); }
                            true
                        } else { false }
                    } else { false };
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
                    let fast = if let Some(vv) = &regs[*var as usize] {
                        if let ValueKind::Int(cur) = vv.kind() {
                            let next = cur.wrapping_add(step);
                            let cont = match cmp_op {
                                BinaryOp::Lt => next < stop,
                                BinaryOp::Gt => next > stop,
                                _ => unreachable!("ForCountConst uses Lt or Gt only"),
                            };
                            if cont { regs[*var as usize] = Some(Value::int(next)); }
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
                    if regs[*reg as usize].is_none() {
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

                    let mut params = Vec::new();
                    let mut def_slot = 0u32;
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
                        id: crate::value::next_fn_id(),
                        name: proto_name,
                        params,
                        local_names: Rc::new(proto_local_index.keys().cloned().collect()),
                        local_index: proto_local_index,
                        global_names: proto_global_names,
                        nonlocal_names: proto_nonlocal_names,
                        env: Rc::clone(&self.env),
                        is_pure,
                        precompiled_code: Some(proto_code),
                    });
                    regs[*dst as usize] = Some(Value::user_function(func));
                }
                Insn::MakeClass(dst, proto_idx, bases_base, bases_n, name_idx) => {
                    let class_name = pool_get!(code.names, *name_idx, "name").clone();
                    let (class_code, local_index) = {
                        let proto = pool_get!(code.fn_protos, *proto_idx, "fn_proto");
                        (Rc::clone(&proto.code), Rc::clone(&proto.local_index))
                    };
                    let num_class_regs = class_code.num_regs as usize;
                    let mut class_regs: Vec<Option<Value>> = vec![None; num_class_regs];
                    vm_try!(self.run_bytecode(&class_code, &mut class_regs));
                    let mut attrs = HashMap::new();
                    for (attr_name, &slot) in local_index.iter() {
                        if let Some(val) = class_regs.get(slot as usize).and_then(|v| v.clone()) {
                            attrs.insert(attr_name.clone(), val);
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
                    regs[*dst as usize] = Some(Value::py_class(class));
                }

                // ── Import ───────────────────────────────────────────────
                Insn::ImportModule(dst, name_idx) => {
                    let name = pool_get!(code.names, *name_idx, "name").clone();
                    let module = vm_try!(self.load_module(&name));
                    regs[*dst as usize] = Some(module);
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

    fn exec_call_method(
        &mut self,
        regs: &mut Vec<Option<Value>>,
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
        // Check if obj is a List, Dict, Tuple, or Str via kind()
        let obj_kind_tag = regs[obj as usize].as_ref().map(|v| match v.kind() {
            ValueKind::List(_) => 1u8,
            ValueKind::Dict(_) => 2u8,
            ValueKind::Tuple(_) => 3u8,
            ValueKind::Str(_) => 4u8,
            _ => 0u8,
        }).unwrap_or(0);

        match obj_kind_tag {
            1 => {
                // as_list_mut is safe: we confirmed tag==List above (single-threaded).
                // obj_reg and dst_reg may coincide; the mutable borrow of the Vec ends
                // before exec_call_method returns, so no alias with the later store.
                let items = regs[obj as usize]
                    .as_mut()
                    .and_then(|v| v.as_list_mut())
                    .ok_or_else(|| PyError::Runtime("internal: expected list".to_string()))?;
                let empty_kw = indexmap::IndexMap::new();
                pyrust_builtins::list::call(&method, items, &args, &empty_kw)
            }
            2 => {
                if matches!(method.as_str(), "keys" | "values" | "items") {
                    let rc = regs[obj as usize]
                        .as_ref()
                        .and_then(|v| v.get_dict_rc())
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
                    .as_mut()
                    .and_then(|v| v.as_dict_mut())
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                pyrust_builtins::dict::call(&method, dict, &args)
            }
            3 => {
                if let Some(ValueKind::Tuple(items)) = regs[obj as usize].as_ref().map(|v| v.kind()) {
                    pyrust_builtins::tuple::call(&method, items, &args)
                } else {
                    unreachable!()
                }
            }
            4 => {
                if let Some(v) = &regs[obj as usize] {
                    pyrust_builtins::string::call(&method, v, &args)
                } else { unreachable!() }
            }
            _ => {
                let obj_val = vm_read(regs, obj, num_locals)?;
                let method_val = self.get_attr(obj_val, &method)?;
                let mut buf = std::mem::take(&mut self.call_arg_buf);
                buf.clear();
                for arg in &args {
                    buf.push(ExpandedCallArg { name: None, value: arg.clone() });
                }
                let r = self.call_function_expanded(method_val, &buf);
                self.call_arg_buf = buf;
                r
            }
        }
    }

    fn exec_call_method_expanded(
        &mut self,
        regs: &mut Vec<Option<Value>>,
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
        let pos_items: Vec<Value> = match vm_read(regs, pos_list, num_locals)? {
            v => match v.kind() {
                ValueKind::List(items) => items.clone(),
                _ => return Err(PyError::Runtime("CallMethodExpanded: pos_list must be a list".to_string())),
            }
        };
        let kw_map = match vm_read(regs, kw_dict, num_locals)? {
            v => match v.kind() {
                ValueKind::Dict(d) => d.clone(),
                _ => return Err(PyError::Runtime("CallMethodExpanded: kw_dict must be a dict".to_string())),
            }
        };

        let obj_kind_tag = regs[obj as usize].as_ref().map(|v| match v.kind() {
            ValueKind::List(_) => 1u8,
            ValueKind::Dict(_) => 2u8,
            ValueKind::Tuple(_) => 3u8,
            ValueKind::Str(_) => 4u8,
            _ => 0u8,
        }).unwrap_or(0);

        match obj_kind_tag {
            1 => {
                // Intercept list.sort here to support key= (needs interpreter access).
                if method == "sort" {
                    for k in kw_map.keys() {
                        if let PyKey::Str(s) = k {
                            if s != "key" && s != "reverse" {
                                return Err(PyError::Named(
                                    "TypeError".to_string(),
                                    format!("sort() got an unexpected keyword argument '{s}'"),
                                ));
                            }
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
                            .as_ref()
                            .and_then(|v| v.as_list())
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
                            .as_mut()
                            .and_then(|v| v.as_list_mut())
                            .ok_or_else(|| PyError::Runtime("internal: expected list".to_string()))?;
                        return pyrust_builtins::list::sort_with_precomputed_keys(items_out, keys, reverse);
                    }
                    // No key: delegate to builtins (handles reverse kwarg)
                    let items = regs[obj as usize]
                        .as_mut()
                        .and_then(|v| v.as_list_mut())
                        .ok_or_else(|| PyError::Runtime("internal: expected list".to_string()))?;
                    return pyrust_builtins::list::call(&method, items, &pos_items, &kw_map);
                }
                let items = regs[obj as usize]
                    .as_mut()
                    .and_then(|v| v.as_list_mut())
                    .ok_or_else(|| PyError::Runtime("internal: expected list".to_string()))?;
                pyrust_builtins::list::call(&method, items, &pos_items, &kw_map)
            }
            2 => {
                if matches!(method.as_str(), "keys" | "values" | "items") {
                    let rc = regs[obj as usize]
                        .as_ref()
                        .and_then(|v| v.get_dict_rc())
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
                    .as_mut()
                    .and_then(|v| v.as_dict_mut())
                    .ok_or_else(|| PyError::Runtime("internal: expected dict".to_string()))?;
                pyrust_builtins::dict::call(&method, dict, &pos_items)
            }
            3 => {
                if let Some(ValueKind::Tuple(items)) = regs[obj as usize].as_ref().map(|v| v.kind()) {
                    pyrust_builtins::tuple::call(&method, items, &pos_items)
                } else {
                    Err(PyError::Runtime("internal: expected tuple".to_string()))
                }
            }
            4 => {
                if let Some(v) = &regs[obj as usize] {
                    pyrust_builtins::string::call(&method, v, &pos_items)
                } else { unreachable!() }
            }
            _ => {
                let obj_val = vm_read(regs, obj, num_locals)?;
                let method_val = self.get_attr(obj_val, &method)?;
                let mut expanded: Vec<ExpandedCallArg> = pos_items
                    .iter()
                    .map(|v| ExpandedCallArg { name: None, value: v.clone() })
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

fn vm_read(regs: &[Option<Value>], reg: crate::bytecode::Reg, num_locals: crate::bytecode::Reg) -> crate::interpreter::Result<Value> {
    match regs[reg as usize].clone() {
        Some(v) => Ok(v),
        None => {
            if reg < num_locals {
                Err(crate::error::PyError::Named(
                    "NameError".to_string(),
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
        UnaryOp::Neg => match val.kind() {
            ValueKind::Int(v) => Ok(Value::int(-v)),
            ValueKind::Float(v) => Ok(Value::float(-v)),
            _ => Err(PyError::Runtime("bad operand type for unary -".to_string())),
        },
        UnaryOp::Not => Ok(Value::bool_(!val.truthy())),
        UnaryOp::BitNot => match val.kind() {
            ValueKind::Int(v) => Ok(Value::int(!v)),
            ValueKind::Bool(b) => Ok(Value::int(if b { -2 } else { -1 })),
            _ => Err(PyError::Runtime(
                "bad operand type for unary ~: use integer".to_string(),
            )),
        },
        UnaryOp::Pos => match val.kind() {
            ValueKind::Int(v) => Ok(Value::int(v)),
            ValueKind::Float(v) => Ok(Value::float(v)),
            ValueKind::Bool(b) => Ok(Value::int(if b { 1 } else { 0 })),
            _ => Err(PyError::Runtime("bad operand type for unary +".to_string())),
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
        let mut regs: Vec<Option<Value>> = vec![None; 1];
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
        let mut regs: Vec<Option<Value>> = vec![];
        let result = interp.run_bytecode(&code, &mut regs);
        assert!(result.is_err(), "expected Err for OOB jump, got {:?}", result);
        assert!(result.unwrap_err().to_string().contains("internal error"));
    }

    #[test]
    fn negative_jump_returns_error() {
        // Jump(-100): new_pc = 1 + (-100) = -99 → underflow error
        let code = empty_code(vec![Insn::Jump(-100)]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Option<Value>> = vec![];
        let result = interp.run_bytecode(&code, &mut regs);
        assert!(result.is_err(), "expected Err for negative jump, got {:?}", result);
        assert!(result.unwrap_err().to_string().contains("internal error"));
    }

    #[test]
    fn normal_fallthrough_returns_none() {
        let code = empty_code(vec![Insn::ReturnNone]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Option<Value>> = vec![];
        assert_eq!(interp.run_bytecode(&code, &mut regs).unwrap(), Value::none());
    }

    #[test]
    fn setup_except_negative_offset_returns_error() {
        // SetupExcept(-100): handler_pc = 1 + (-100) < 0 → error at push time
        let code = empty_code(vec![Insn::SetupExcept(-100), Insn::ReturnNone]);
        let mut interp = Interpreter::default();
        let mut regs: Vec<Option<Value>> = vec![];
        let result = interp.run_bytecode(&code, &mut regs);
        assert!(result.is_err(), "expected Err for SetupExcept with OOB offset, got {:?}", result);
    }
}
