// Register/cell placement for already-bound Python function parameters.

#[inline(always)]
fn bind_param(
    bound: &mut [bool],
    function: &Rc<UserFunction>,
    num_regs: usize,
    regs: &mut RegsBuf,
    cells: &CellsBuf,
    pi: usize,
    val: Value,
) -> Result<()> {
    bound[pi] = true;
    bind_param_direct(function, num_regs, regs, cells, pi, val)
}

#[inline]
pub(crate) fn bind_param_direct(
    function: &Rc<UserFunction>,
    num_regs: usize,
    regs: &mut RegsBuf,
    cells: &CellsBuf,
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
        pyrust_core::ParamBind::Cell(cell_idx) => {
            cells[cell_idx as usize].set(val);
        }
        pyrust_core::ParamBind::None => {}
    }
    Ok(())
}
