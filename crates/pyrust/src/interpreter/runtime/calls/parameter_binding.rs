// Register/cell placement for already-bound Python function parameters.

#[inline(always)]
fn bind_param(
    bound: &mut [bool],
    function: &Rc<UserFunction>,
    num_regs: usize,
    regs: &mut RegsBuf,
    local_env: &Option<EnvRef>,
    pi: usize,
    val: Value,
) -> Result<()> {
    bound[pi] = true;
    bind_param_direct(function, num_regs, regs, local_env, pi, val)
}

#[inline]
pub(crate) fn bind_param_direct(
    function: &Rc<UserFunction>,
    num_regs: usize,
    regs: &mut RegsBuf,
    local_env: &Option<EnvRef>,
    pi: usize,
    val: Value,
) -> Result<()> {
    match function.param_binds[pi] {
        pyrust_core::ParamBind::Reg(reg) => {
            if reg as usize >= num_regs {
                return Err(pyrust_core::py_err!(
                    "SystemError",
                    "parameter '{}' register index {} out of range (num_regs={})",
                    function.params[pi].name,
                    reg,
                    num_regs
                ));
            }
            regs[reg as usize] = val;
        }
        pyrust_core::ParamBind::Cell => {
            if let Some(env) = local_env {
                env.borrow_mut()
                    .values
                    .insert(&function.params[pi].name, val);
            }
        }
        pyrust_core::ParamBind::None => {}
    }
    Ok(())
}
