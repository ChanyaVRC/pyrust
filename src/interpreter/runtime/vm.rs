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
        let num_locals = code.num_locals;

        let mut iters: Vec<Option<(Vec<Value>, usize)>> =
            vec![None; code.num_iters as usize];

        let mut pc: usize = 0;

        loop {
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
                    let val = if let Some(v) = self.lookup_name(name)? {
                        v
                    } else {
                        resolve_builtin(name).ok_or_else(|| {
                            PyError::Runtime(format!("name '{}' is not defined", name))
                        })?
                    };
                    regs[*dst as usize] = Some(val);
                }
                Insn::LoadNone(dst) => {
                    regs[*dst as usize] = Some(Value::None);
                }
                Insn::Move(dst, src) => {
                    let v = vm_read(regs, *src, num_locals)?;
                    regs[*dst as usize] = Some(v);
                }

                // ── Arithmetic / Logic ───────────────────────────────────
                Insn::BinOp(dst, lhs, op, rhs) => {
                    let l = vm_read(regs, *lhs, num_locals)?;
                    let r = vm_read(regs, *rhs, num_locals)?;
                    let result = self.eval_binary(l, *op, r)?;
                    regs[*dst as usize] = Some(result);
                }
                Insn::UnaryOp(dst, op, src) => {
                    let val = vm_read(regs, *src, num_locals)?;
                    let result = vm_eval_unary(*op, val)?;
                    regs[*dst as usize] = Some(result);
                }

                // ── Attribute / Index ────────────────────────────────────
                Insn::GetAttr(dst, obj, name_idx) => {
                    let obj_val = vm_read(regs, *obj, num_locals)?;
                    let name = &code.names[*name_idx as usize];
                    let result = self.get_attr(obj_val, name)?;
                    regs[*dst as usize] = Some(result);
                }
                Insn::SetAttr(obj, name_idx, val) => {
                    let obj_val = vm_read(regs, *obj, num_locals)?;
                    let val_val = vm_read(regs, *val, num_locals)?;
                    let name = &code.names[*name_idx as usize];
                    self.assign_attr(obj_val, name, val_val)?;
                }
                Insn::GetItem(dst, obj, idx) => {
                    let obj_val = vm_read(regs, *obj, num_locals)?;
                    let idx_val = vm_read(regs, *idx, num_locals)?;
                    let result = self.eval_index(obj_val, idx_val)?;
                    regs[*dst as usize] = Some(result);
                }
                Insn::SetItem(obj, idx, val) => {
                    let idx_val = vm_read(regs, *idx, num_locals)?;
                    let val_val = vm_read(regs, *val, num_locals)?;
                    match regs[*obj as usize].as_mut() {
                        Some(Value::List(items)) => {
                            let i = normalize_index(&idx_val, items.len())?;
                            items[i] = val_val;
                        }
                        Some(Value::Dict(dict)) => {
                            let key = idx_val.to_key().ok_or_else(|| {
                                PyError::Runtime("unhashable type".to_string())
                            })?;
                            dict.insert(key, val_val);
                        }
                        _ => {
                            return Err(PyError::Runtime(
                                "object does not support item assignment".to_string(),
                            ));
                        }
                    }
                }

                // ── Control flow ─────────────────────────────────────────
                Insn::Jump(offset) => {
                    pc = (pc as i32 + offset) as usize;
                }
                Insn::JumpIfFalse(cond, offset) => {
                    if !vm_read(regs, *cond, num_locals)?.truthy() {
                        pc = (pc as i32 + offset) as usize;
                    }
                }
                Insn::JumpIfTrue(cond, offset) => {
                    if vm_read(regs, *cond, num_locals)?.truthy() {
                        pc = (pc as i32 + offset) as usize;
                    }
                }

                // ── Calls ────────────────────────────────────────────────
                Insn::Call(func_reg, argc) => {
                    let func_val = vm_read(regs, *func_reg, num_locals)?;
                    let mut args: Vec<ExpandedCallArg> = Vec::with_capacity(*argc as usize);
                    for i in 0..*argc as usize {
                        args.push(ExpandedCallArg {
                            name: None,
                            value: vm_read(regs, *func_reg + 1 + i as u8, num_locals)?,
                        });
                    }
                    let result = self.call_function_expanded(func_val, &args)?;
                    regs[*func_reg as usize] = Some(result);
                }

                // ── Returns ──────────────────────────────────────────────
                Insn::Return(src) => {
                    return Ok(vm_read(regs, *src, num_locals)?);
                }
                Insn::ReturnNone => {
                    return Ok(Value::None);
                }

                // ── Collection builders ──────────────────────────────────
                Insn::BuildList(dst, base, n) => {
                    let items = (0..*n as usize)
                        .map(|i| vm_read(regs, *base + i as u8, num_locals))
                        .collect::<Result<Vec<_>>>()?;
                    regs[*dst as usize] = Some(Value::List(items));
                }
                Insn::BuildTuple(dst, base, n) => {
                    let items = (0..*n as usize)
                        .map(|i| vm_read(regs, *base + i as u8, num_locals))
                        .collect::<Result<Vec<_>>>()?;
                    regs[*dst as usize] = Some(Value::Tuple(items));
                }
                Insn::BuildDict(dst, base, n) => {
                    let mut dict = indexmap::IndexMap::new();
                    for i in 0..*n as usize {
                        let k_val = vm_read(regs, *base + (i * 2) as u8, num_locals)?;
                        let v_val = vm_read(regs, *base + (i * 2 + 1) as u8, num_locals)?;
                        let key = k_val.to_key().ok_or_else(|| {
                            PyError::Runtime("unhashable type in dict key".to_string())
                        })?;
                        dict.insert(key, v_val);
                    }
                    regs[*dst as usize] = Some(Value::Dict(dict));
                }

                // ── Unpack ───────────────────────────────────────────────
                Insn::Unpack(base, src, n) => {
                    let src_val = vm_read(regs, *src, num_locals)?;
                    let items = iter_values(src_val)?;
                    if items.len() != *n as usize {
                        return Err(PyError::Runtime(format!(
                            "not enough values to unpack (expected {}, got {})",
                            n,
                            items.len()
                        )));
                    }
                    for (i, v) in items.into_iter().enumerate() {
                        regs[*base as usize + i] = Some(v);
                    }
                }

                // ── Iterator ─────────────────────────────────────────────
                Insn::GetIter(slot, src) => {
                    let src_val = vm_read(regs, *src, num_locals)?;
                    let items = iter_values(src_val)?;
                    iters[*slot as usize] = Some((items, 0));
                }
                Insn::CheckLocal(reg, name_idx) => {
                    if regs[*reg as usize].is_none() {
                        let name = &code.names[*name_idx as usize];
                        return Err(crate::error::PyError::Runtime(format!(
                            "cannot access local variable '{}' where it is not associated with a value",
                            name
                        )));
                    }
                }
                Insn::ForIter(dst, slot, offset) => {
                    if let Some((items, pos)) = iters[*slot as usize].as_mut() {
                        if *pos < items.len() {
                            let v = items[*pos].clone();
                            *pos += 1;
                            regs[*dst as usize] = Some(v);
                        } else {
                            // Iterator exhausted: jump past the loop
                            pc = (pc as i32 + offset) as usize;
                        }
                    } else {
                        pc = (pc as i32 + offset) as usize;
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
