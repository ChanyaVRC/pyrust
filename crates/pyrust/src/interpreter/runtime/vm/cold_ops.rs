// Cold/slow VM opcode dispatcher and helpers: collection builders, call-splat
// accumulators, formatting, class/function/PEP-695 creation, imports, and REPL.

#[inline(always)]
fn vm_pool_get<'a, T>(pool: &'a [T], idx: u16, tag: &str) -> Result<&'a T> {
    pool.get(idx as usize).ok_or_else(|| {
        PyError::Runtime(format!(
            "bytecode error: {} index {} out of range (pool size {})",
            tag,
            idx,
            pool.len()
        ))
    })
}

impl Interpreter {
    /// Out-of-line dispatcher for all cold/slow VM opcodes.
    ///
    /// Keeping these arms outside `run_bytecode_inner_impl` shrinks the hot
    /// dispatch loop's stack frame and I-cache footprint while preserving exact
    /// error propagation and jump/yield semantics.
    #[allow(clippy::too_many_arguments)]
    #[cold]
    #[inline(never)]
    fn exec_cold_insn(
        &mut self,
        insn: &crate::bytecode::Insn,
        code: &crate::bytecode::FnCode,
        regs: &mut RegSlice,
        cells: CellSlice,
        pc: usize,
        cur_line: u32,
        num_locals: crate::bytecode::Reg,
        exc_ctx_frame_base: usize,
        exc_saved_active_frame_base: usize,
    ) -> Result<ColdInsnOutcome> {
        use crate::bytecode::Insn;
        if let Some(outcome) = self.exec_cold_control_insn(
            insn,
            regs,
            num_locals,
            exc_ctx_frame_base,
            exc_saved_active_frame_base,
        )? {
            return Ok(outcome);
        }
        if self.exec_cold_match_insn(insn, regs, num_locals)? {
            return Ok(ColdInsnOutcome::Continue);
        }
        match insn {
            // ── Cold loads / namespace / attribute / type-param ──────────
            Insn::LoadNoneRange { start, count } => {
                let s = *start as usize;
                for idx in s..(s + *count as usize) {
                    regs[idx] = Value::none();
                }
            }
            Insn::DeleteItem(obj, idx) => {
                self.exec_delete_item(regs, num_locals, *obj, *idx)?;
            }
            Insn::LoadClassName(dst, local, name_idx) => {
                regs[*dst as usize] = self.load_class_name(code, regs, *local, *name_idx)?;
            }
            Insn::DeleteCell(cell_idx, raise_if_missing) => {
                self.exec_delete_cell(code, regs, cells, *cell_idx, *raise_if_missing)?;
            }
            Insn::LoadClassDeref(dst, cell_idx) => {
                regs[*dst as usize] = self.load_class_deref(code, cells, *cell_idx)?;
            }
            Insn::GetAttrForWith(dst, obj, name_idx, proto) => {
                regs[*dst as usize] =
                    self.exec_get_attr_for_with(regs, *obj, *name_idx, *proto, code, num_locals)?;
            }
            Insn::ImportFromAttr(dst, mod_reg, name_idx) => {
                regs[*dst as usize] =
                    self.exec_import_from_attr(regs, *mod_reg, *name_idx, code, num_locals)?;
            }
            Insn::DeleteAttr(obj, name_idx) => {
                self.exec_delete_attr(regs, *obj, *name_idx, code, num_locals)?;
            }
            Insn::SetTypeVarAttr(obj, name_idx, val) => {
                Self::exec_set_typevar_attr(regs, *obj, *name_idx, *val, code, num_locals)?;
            }
            Insn::DeleteName(name_idx) => {
                self.exec_delete_name(code, regs, *name_idx)?;
            }
            Insn::PushTypeParamEnv => self.push_type_parameter_scope(),
            Insn::PopTypeParamEnv => self.pop_type_parameter_scope(),
            Insn::DeleteLocal(reg, name_idx, raise_if_missing) => {
                self.delete_local_binding(code, regs, *reg, *name_idx, *raise_if_missing)?;
            }
            Insn::SyncModuleGlobal(reg, name_idx) => {
                self.sync_module_global_binding(code, regs, *reg, *name_idx)?;
            }
            Insn::DeleteModuleGlobal(name_idx) => {
                self.delete_module_global_binding(code, regs, *name_idx)?;
            }

            // ── Keyword / splat calls ────────────────────────────────────
            Insn::CallKw {
                func,
                total,
                nkw,
                kwnames_idx,
            } => {
                regs[*func as usize] = self.exec_call_kw(
                    regs,
                    code,
                    pc,
                    *func,
                    *total,
                    *nkw,
                    *kwnames_idx,
                    num_locals,
                )?;
            }
            Insn::CallEx { func, npos, kwargs } => {
                regs[*func as usize] =
                    self.exec_call_ex(regs, code, pc, *func, *npos, *kwargs, num_locals)?;
            }
            Insn::CallExArgs {
                func,
                npos,
                nkw,
                kwnames_idx,
                args_splat,
                kwargs,
            } => {
                regs[*func as usize] = self.exec_call_ex_args(
                    regs,
                    code,
                    pc,
                    *func,
                    *npos,
                    *nkw,
                    *kwnames_idx,
                    *args_splat,
                    *kwargs,
                    num_locals,
                )?;
            }
            Insn::CallMethodExpanded {
                dst,
                obj,
                name_idx,
                pos_list,
                kw_dict,
            } => {
                regs[*dst as usize] = self.exec_call_method_expanded(
                    regs, num_locals, *dst, *obj, *name_idx, *pos_list, *kw_dict, code,
                )?;
            }
            Insn::CallMethodKw {
                dst,
                obj,
                name_idx,
                args_base,
                total,
                nkw,
                kwnames_idx,
            } => {
                regs[*dst as usize] = self.exec_call_method_kw(
                    regs,
                    num_locals,
                    *dst,
                    *obj,
                    *name_idx,
                    *args_base,
                    *total,
                    *nkw,
                    *kwnames_idx,
                    code,
                    pc - 1,
                    cur_line,
                )?;
            }

            // ── Collection builders & formatting ─────────────────────────
            Insn::BuildList(dst, base, n) => {
                regs[*dst as usize] = Self::exec_build_list(regs, *base, *n, num_locals)?;
            }
            Insn::BuildListReserve(dst, src) => {
                regs[*dst as usize] = Self::exec_build_list_reserve(regs, *src, num_locals)?;
            }
            Insn::BuildTuple(dst, base, n) => {
                regs[*dst as usize] = Self::exec_build_tuple(regs, *base, *n, num_locals)?;
            }
            Insn::BuildString(dst, base, n) => {
                regs[*dst as usize] = build_string_fast(regs, *base, *n, num_locals)?;
            }
            Insn::FormatValue(dst, src) => {
                regs[*dst as usize] = self.exec_format_value(regs, *src, num_locals)?;
            }
            Insn::FormatValueSpec(dst, src, spec_r) => {
                regs[*dst as usize] =
                    self.exec_format_value_spec(regs, *src, *spec_r, code, pc - 1, num_locals)?;
            }
            Insn::BuildSlice(dst, base) => {
                regs[*dst as usize] = Self::exec_build_slice(regs, *base, num_locals)?;
            }
            Insn::BuildDict(dst, base, n, key_kind_hint) => {
                regs[*dst as usize] =
                    self.exec_build_dict(regs, *base, *n, *key_kind_hint, num_locals)?;
            }
            Insn::SetAdd(set_reg, val_reg) => {
                self.exec_set_add(regs, *set_reg, *val_reg, num_locals)?;
            }
            Insn::ListExtend(list_reg, src_reg) => {
                self.exec_list_extend(regs, *list_reg, *src_reg, num_locals)?;
            }
            Insn::ListExtendCall { list, src, name } => {
                self.exec_list_extend_call(regs, *list, *src, name, code, num_locals)?;
            }
            Insn::DictUpdate(dict_reg, src_reg) => {
                self.exec_dict_update(regs, *dict_reg, *src_reg, num_locals)?;
            }
            Insn::DictMergeKwCall { dict, src, name } => {
                self.exec_dict_merge_kw_call(regs, *dict, *src, name, code, num_locals)?;
            }
            Insn::SetItemKwCall {
                dict,
                key,
                val,
                name,
            } => {
                self.exec_set_item_kw_call(regs, *dict, *key, *val, name, code, num_locals)?;
            }

            // ── Function / Class / PEP 695 / Import / REPL ───────────────
            Insn::MakeFunction(dst, proto_idx, defs_base, _defs_n, annots_base, _annots_n) => {
                regs[*dst as usize] = self.exec_make_function(
                    code,
                    regs,
                    cells,
                    num_locals,
                    *proto_idx,
                    *defs_base,
                    *annots_base,
                )?;
            }
            Insn::MakeClass(dst, proto_idx, bases_base, bases_n, name_idx, kwarg_base, kwarg_n) => {
                regs[*dst as usize] = self.exec_make_class(
                    code,
                    regs,
                    cells,
                    num_locals,
                    *proto_idx,
                    *bases_base,
                    *bases_n,
                    *name_idx,
                    *kwarg_base,
                    *kwarg_n,
                )?;
            }
            Insn::MakeClassMeta(
                dst,
                proto_idx,
                bases_base,
                bases_n,
                name_idx,
                kwarg_base,
                kwarg_n,
                meta_reg,
            ) => {
                regs[*dst as usize] = self.exec_make_class_meta(
                    code,
                    regs,
                    cells,
                    num_locals,
                    *proto_idx,
                    *bases_base,
                    *bases_n,
                    *name_idx,
                    *kwarg_base,
                    *kwarg_n,
                    *meta_reg,
                )?;
            }
            Insn::MakeTypeVar(dst, name_idx) => {
                regs[*dst as usize] = Self::exec_make_typevar(code, *name_idx)?;
            }
            Insn::MakeTypeAlias(dst, name_idx, value_reg, params_reg) => {
                regs[*dst as usize] = self.exec_make_type_alias(
                    regs,
                    *name_idx,
                    *value_reg,
                    *params_reg,
                    code,
                    num_locals,
                )?;
            }
            Insn::ImportModule(dst, name_idx) => {
                regs[*dst as usize] = self.exec_import_module(code, *name_idx)?;
            }
            Insn::ImportStar(mod_reg) => {
                self.exec_import_star(regs, num_locals, *mod_reg)?;
            }
            Insn::PrintExpr(src) => {
                Self::exec_print_expr(regs, *src, num_locals)?;
            }
            Insn::RecordClassStore(slot) => self.record_class_namespace_store(regs, *slot),
            Insn::RecordClassDel(slot) => self.record_class_namespace_delete(*slot),
            _ => unreachable!("hot VM opcode dispatched in run_bytecode_inner_impl"),
        }
        Ok(ColdInsnOutcome::Continue)
    }

    fn exec_build_list(
        regs: &RegSlice,
        base: crate::bytecode::Reg,
        n: u32,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let mut items: Vec<Value> = Vec::with_capacity(n as usize);
        for i in 0..n {
            items.push(vm_read(regs, base + i, num_locals)?);
        }
        Ok(Value::list(items))
    }

    fn exec_build_list_reserve(
        regs: &RegSlice,
        src: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let src_val = vm_read(regs, src, num_locals)?;
        let cap = list_reserve_hint(&src_val);
        Ok(Value::list(Vec::with_capacity(cap)))
    }

    fn exec_build_tuple(
        regs: &RegSlice,
        base: crate::bytecode::Reg,
        n: u32,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let mut items = Vec::with_capacity(n as usize);
        for i in 0..n {
            items.push(vm_read(regs, base + i, num_locals)?);
        }
        Ok(Value::tuple(items))
    }

    fn exec_format_value(
        &mut self,
        regs: &RegSlice,
        src: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let val = vm_read(regs, src, num_locals)?;
        self.format_value_default(&val)
    }

    fn exec_format_value_spec(
        &mut self,
        regs: &RegSlice,
        src: crate::bytecode::Reg,
        spec_r: crate::bytecode::Reg,
        code: &crate::bytecode::FnCode,
        site_pc: usize,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let val = vm_read(regs, src, num_locals)?;
        let spec_val = vm_read(regs, spec_r, num_locals)?;
        self.format_value_spec_cached(&val, &spec_val, &code.fmt_spec_cache, site_pc)
    }

    fn exec_build_slice(
        regs: &RegSlice,
        base: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let start = vm_read(regs, base, num_locals)?;
        let stop = vm_read(regs, base + 1, num_locals)?;
        let step = vm_read(regs, base + 2, num_locals)?;
        let lo = if start.is_none() { None } else { Some(start) };
        let hi = if stop.is_none() { None } else { Some(stop) };
        let st = if step.is_none() { None } else { Some(step) };
        Ok(make_slice_value(lo, hi, st))
    }

    fn exec_build_dict(
        &mut self,
        regs: &RegSlice,
        base: crate::bytecode::Reg,
        n: u32,
        key_kind_hint: crate::bytecode::DictKeyKindHint,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let unicode_only = match key_kind_hint {
            crate::bytecode::DictKeyKindHint::Unicode => true,
            crate::bytecode::DictKeyKindHint::General => false,
            crate::bytecode::DictKeyKindHint::Unknown => {
                let mut unicode_only = true;
                for offset in 0..n {
                    let key_reg = base + offset * 2;
                    let key = &regs[key_reg as usize];
                    if key.is_unset() {
                        let _ = vm_read(regs, key_reg, num_locals)?;
                        unreachable!("vm_read returned an unset register");
                    }
                    if !key.is_str() {
                        unicode_only = false;
                        break;
                    }
                }
                unicode_only
            }
        };
        let pairs = (0..n).map(|offset| {
            let key = vm_read(regs, base + offset * 2, num_locals)?;
            let value = vm_read(regs, base + offset * 2 + 1, num_locals)?;
            Ok((key, value))
        });
        self.dict_from_value_pairs(n as usize, unicode_only, pairs)
    }

    fn exec_set_add(
        &mut self,
        regs: &RegSlice,
        set_reg: crate::bytecode::Reg,
        val_reg: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let set = vm_read(regs, set_reg, num_locals)?;
        let val = vm_read(regs, val_reg, num_locals)?;
        self.set_insert_value(&set, val)
    }

    fn exec_list_extend(
        &mut self,
        regs: &RegSlice,
        list_reg: crate::bytecode::Reg,
        src_reg: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let list = vm_read(regs, list_reg, num_locals)?;
        let src_val = vm_read(regs, src_reg, num_locals)?;
        self.extend_list_accumulator(&list, &src_val)
    }

    fn exec_list_extend_call(
        &mut self,
        regs: &RegSlice,
        list: crate::bytecode::Reg,
        src: crate::bytecode::Reg,
        name: &crate::bytecode::KwCallName,
        code: &crate::bytecode::FnCode,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let list_val = vm_read(regs, list, num_locals)?;
        let src_val = vm_read(regs, src, num_locals)?;
        if let Err(error) = self.extend_list_accumulator(&list_val, &src_val) {
            let error = star_operand_error(&src_val, error, || {
                self.kwcall_func_name(regs, num_locals, name, code)
            });
            return Err(error);
        }
        Ok(())
    }

    fn exec_dict_update(
        &mut self,
        regs: &RegSlice,
        dict_reg: crate::bytecode::Reg,
        src_reg: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let dict = vm_read(regs, dict_reg, num_locals)?;
        let src_val = vm_read(regs, src_reg, num_locals)?;
        self.update_dict_accumulator(&dict, &src_val)
    }

    fn exec_dict_merge_kw_call(
        &mut self,
        regs: &RegSlice,
        dict: crate::bytecode::Reg,
        src: crate::bytecode::Reg,
        name: &crate::bytecode::KwCallName,
        code: &crate::bytecode::FnCode,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let src_val = vm_read(regs, src, num_locals)?;
        let dict_val = regs[dict as usize]
            .as_some()
            .cloned()
            .unwrap_or(Value::none());
        match self.merge_kwcall_mapping(&dict_val, &src_val)? {
            KwCallMappingMerge::Complete => Ok(()),
            KwCallMappingMerge::Duplicate(keyword) => {
                let fname = self.kwcall_func_name(regs, num_locals, name, code);
                Err(duplicate_keyword_error(fname.as_deref(), &keyword))
            }
            KwCallMappingMerge::NotMapping => {
                let fname = self.kwcall_func_name(regs, num_locals, name, code);
                Err(keyword_mapping_operand_error(fname.as_deref(), &src_val))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn exec_set_item_kw_call(
        &mut self,
        regs: &RegSlice,
        dict: crate::bytecode::Reg,
        key: crate::bytecode::Reg,
        val: crate::bytecode::Reg,
        name: &crate::bytecode::KwCallName,
        code: &crate::bytecode::FnCode,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let key_val = vm_read(regs, key, num_locals)?;
        let val_val = vm_read(regs, val, num_locals)?;
        let dict_val = regs[dict as usize]
            .as_some()
            .cloned()
            .unwrap_or(Value::none());
        if let Some(kw) = self.set_kwcall_value(&dict_val, &key_val, val_val)? {
            let fname = self.kwcall_func_name(regs, num_locals, name, code);
            return Err(duplicate_keyword_error(fname.as_deref(), &kw));
        }
        Ok(())
    }

    fn exec_get_attr_for_with(
        &mut self,
        regs: &RegSlice,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        proto: u8,
        code: &crate::bytecode::FnCode,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let obj_val = vm_read(regs, obj, num_locals)?;
        let name = vm_pool_get(&code.names, name_idx, "name")?;
        self.get_required_protocol_attr(&obj_val, name, proto)
    }

    fn exec_import_from_attr(
        &mut self,
        regs: &RegSlice,
        mod_reg: crate::bytecode::Reg,
        name_idx: u16,
        code: &crate::bytecode::FnCode,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let mod_val = vm_read(regs, mod_reg, num_locals)?;
        let name = vm_pool_get(&code.names, name_idx, "name")?;
        self.import_from_attribute(&mod_val, name)
    }

    fn exec_delete_attr(
        &mut self,
        regs: &RegSlice,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        code: &crate::bytecode::FnCode,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let obj_val = vm_read(regs, obj, num_locals)?;
        let name = vm_pool_get(&code.names, name_idx, "name")?;
        self.delete_attr(obj_val, name)
    }

    fn exec_set_typevar_attr(
        regs: &RegSlice,
        obj: crate::bytecode::Reg,
        name_idx: u16,
        val: crate::bytecode::Reg,
        code: &crate::bytecode::FnCode,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let obj_val = vm_read(regs, obj, num_locals)?;
        let val = vm_read(regs, val, num_locals)?;
        let name = vm_pool_get(&code.names, name_idx, "name")?;
        initialize_typevar_attr(&obj_val, name, val);
        Ok(())
    }

    fn exec_make_typevar(code: &crate::bytecode::FnCode, name_idx: u16) -> Result<Value> {
        let name_val = vm_pool_get(&code.consts, name_idx, "const")?;
        make_typevar_from_syntax(name_val)
    }

    fn exec_make_type_alias(
        &mut self,
        regs: &RegSlice,
        name_idx: u16,
        value_reg: crate::bytecode::Reg,
        params_reg: crate::bytecode::Reg,
        code: &crate::bytecode::FnCode,
        num_locals: crate::bytecode::Reg,
    ) -> Result<Value> {
        let name_val = vm_pool_get(&code.consts, name_idx, "const")?;
        let value_val = vm_read(regs, value_reg, num_locals)?;
        let params_val = vm_read(regs, params_reg, num_locals)?;
        self.register_class_annotation_evaluator(&value_val, regs);
        let module_name = self.defining_module_name()?;
        make_type_alias_from_syntax(name_val, value_val, params_val, module_name)
    }

    fn exec_import_module(
        &mut self,
        code: &crate::bytecode::FnCode,
        name_idx: u16,
    ) -> Result<Value> {
        let name = vm_pool_get(&code.names, name_idx, "name")?;
        self.load_module(name)
    }

    fn exec_print_expr(
        regs: &RegSlice,
        src: crate::bytecode::Reg,
        num_locals: crate::bytecode::Reg,
    ) -> Result<()> {
        let val = vm_read(regs, src, num_locals)?;
        if !val.is_none() {
            println!("{}", val.repr_raw());
        }
        Ok(())
    }
}
