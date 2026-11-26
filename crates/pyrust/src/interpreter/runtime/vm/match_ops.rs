// Pattern-matching and sequence-unpacking opcode helpers.

impl Interpreter {
    /// Dispatch pattern-matching and unpacking cold opcodes.
    /// Returns `Ok(true)` if handled, `Ok(false)` otherwise.
    fn exec_cold_match_insn(
        &mut self,
        insn: &crate::bytecode::Insn,
        code: &crate::bytecode::FnCode,
        regs: &mut RegSlice,
        num_locals: crate::bytecode::Reg,
    ) -> Result<bool> {
        use crate::bytecode::Insn;
        match insn {
            Insn::MatchSeqExcluded(dst, subj) => {
                regs[*dst as usize] = Self::exec_match_seq_excluded(regs, *subj, num_locals)?;
                Ok(true)
            }
            Insn::MatchMapping(dst, subj) => {
                regs[*dst as usize] = Self::exec_match_mapping(regs, *subj, num_locals)?;
                Ok(true)
            }
            Insn::MatchClassPositional(desc_idx) => {
                let desc = code.descs[*desc_idx as usize].as_match_class_positional();
                self.exec_match_class_positional(regs, code, desc, num_locals)?;
                Ok(true)
            }
            Insn::Unpack(base, src, n) => {
                self.exec_unpack(regs, *base, *src, *n, num_locals)?;
                Ok(true)
            }
            Insn::UnpackEx {
                src,
                before,
                after,
                dst_base,
            } => {
                self.exec_unpack_ex(regs, *src, *before, *after, *dst_base, num_locals)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// `isinstance(subj, (str, bytes, dict, set, frozenset))` — sequence-pattern
    /// type exclusion (#1789).
    fn exec_match_seq_excluded(
        regs: &RegSlice,
        subj: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let subj_val = vm_read(regs, subj, num_locals)?;
        Ok(Value::bool_(crate::interpreter::value_is_seq_excluded(
            &subj_val,
        )))
    }

    /// `isinstance(subj, collections.abc.Mapping)` — mapping-pattern type gate (#1879).
    fn exec_match_mapping(
        regs: &RegSlice,
        subj: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let subj_val = vm_read(regs, subj, num_locals)?;
        Ok(Value::bool_(crate::interpreter::value_is_mapping(
            &subj_val,
        )))
    }

    fn exec_match_class_positional(
        &mut self,
        regs: &mut RegSlice,
        code: &crate::bytecode::FnCode,
        desc: &crate::bytecode::MatchClassPositionalDesc,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let class = vm_read(regs, desc.cls, num_locals)?;
        let subject = vm_read(regs, desc.subj, num_locals)?;
        let kw_names: Vec<String> = if desc.nkw == 0 {
            Vec::new()
        } else {
            let names = code
                .consts
                .get(desc.kwnames_idx as usize)
                .and_then(|value| value.as_tuple())
                .ok_or_else(|| {
                    pyrust_core::type_err!("bytecode error: MatchClass kwnames is not a tuple")
                })?;
            names
                .iter()
                .map(|name| name.as_str().map(str::to_owned))
                .collect::<Option<_>>()
                .ok_or_else(|| {
                    pyrust_core::type_err!("bytecode error: MatchClass kwnames must be strings")
                })?
        };
        let values =
            self.match_class_attribute_values(&subject, &class, desc.n as usize, &kw_names)?;
        let found = values.is_some();
        for (offset, value) in values.into_iter().flatten().enumerate() {
            regs[desc.dst_base as usize + offset] = value;
        }
        regs[desc.found_reg() as usize] = Value::bool_(found);
        Ok(())
    }

    fn exec_unpack(
        &mut self,
        regs: &mut RegSlice,
        base: crate::bytecode::Reg,
        src: crate::bytecode::Reg,
        n: u32,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let src_val = vm_read(regs, src, num_locals)?;
        let values = self.unpack_exact_values(&src_val, n as usize)?;
        store_register_values(regs, base, values, "Unpack")
    }

    fn exec_unpack_ex(
        &mut self,
        regs: &mut RegSlice,
        src: crate::bytecode::Reg,
        before: u8,
        after: u32,
        dst_base: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let src_val = vm_read(regs, src, num_locals)?;
        let values = self.unpack_extended_values(&src_val, before as usize, after as usize)?;
        store_register_values(regs, dst_base, values, "UnpackEx")
    }
}
