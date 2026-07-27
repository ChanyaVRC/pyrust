pub(super) fn vm_read(
    regs: &[Value],
    reg: crate::bytecode::Reg,
    num_locals: crate::bytecode::Reg,
) -> crate::interpreter::Result<Value> {
    let v = &regs[reg as usize];
    if v.is_unset() {
        if reg < num_locals {
            return Err(pyrust_core::py_err!(
                "NameError",
                "local variable referenced before assignment"
            ));
        } else {
            return Err(crate::error::PyError::Runtime(
                "internal: temp register read before write".to_string(),
            ));
        }
    }
    Ok(v.clone())
}

fn store_register_values(
    regs: &mut RegSlice,
    base: crate::bytecode::Reg,
    values: Vec<Value>,
    opcode: &str,
) -> Result<()> {
    for (offset, value) in values.into_iter().enumerate() {
        let destination = base as usize + offset;
        if destination >= regs.len() {
            return Err(PyError::Runtime(format!(
                "{opcode}: register {destination} out of range"
            )));
        }
        regs[destination] = value;
    }
    Ok(())
}

impl Interpreter {
    /// Resolve the bytecode callee to the call-runtime name used by duplicate
    /// keyword diagnostics. Register decoding stays in the VM; name rendering
    /// stays in `calls`.
    pub(crate) fn kwcall_func_name(
        &self,
        regs: &RegSlice,
        num_locals: crate::bytecode::Reg,
        name: &crate::bytecode::KwCallName,
        code: &crate::bytecode::FnCode,
    ) -> Option<String> {
        match name {
            crate::bytecode::KwCallName::Callee(reg) => {
                let callee = vm_read(regs, *reg, num_locals).ok()?;
                callable_error_name(&callee)
            }
            crate::bytecode::KwCallName::Method { obj, name_idx } => {
                let receiver = vm_read(regs, *obj, num_locals).ok()?;
                let method = code.names.get(*name_idx as usize)?.as_str();
                let class = match receiver.kind() {
                    ValueKind::PyInstance(instance) => Rc::clone(&instance.borrow().class),
                    ValueKind::PyClass(class) => Rc::clone(class),
                    _ => return None,
                };
                let unbound = lookup_class_attr(&class, method)?;
                callable_error_name(&unbound)
            }
        }
    }
}
