impl Compiler {
    fn compile_lambda(&mut self, params: &[FunctionParam], body: &Expr) -> Reg {
        // A lambda shares function-prototype/default/annotation semantics with
        // `def`, but has no source-level binding. Build its function value
        // directly instead of routing it through an impossible `<lambda>`
        // global name; that keeps compiler transport out of Python namespaces
        // and avoids StoreGlobal/LoadGlobal/DeleteName on every lambda.
        let body_stmts = vec![Stmt::Return(Some(body.clone()))];
        let Some((proto_idx, _, _)) = self.build_def_proto(
            "<lambda>",
            params,
            &body_stmts,
            &[],
            self.current_lineno,
            None,
            false,
        ) else {
            return 0;
        };
        let Some((defs_base, defs_n)) = self.emit_def_default_values(params) else {
            return 0;
        };
        let Some((annots_base, annots_n)) = self.emit_def_annotation_values(params, None) else {
            return 0;
        };

        let dst = self.alloc_temp();
        self.emit(Insn::MakeFunction(
            dst,
            proto_idx,
            defs_base,
            defs_n,
            annots_base,
            annots_n,
        ));

        // Defaults/annotations are dead after MakeFunction. Compact the result
        // into the first register they reserved so callers do not carry holes
        // in the temporary stack.
        if dst != defs_base {
            self.emit(Insn::Move(defs_base, dst));
        }
        self.next_temp = defs_base + 1;
        defs_base
    }

    fn compile_collection(&mut self, items: &[Expr], is_tuple: bool) -> Reg {
        let n = items.len() as Reg;
        let base = self.next_temp;
        if base.checked_add(n).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some(format!(
                    "too many elements in {} literal",
                    if is_tuple { "tuple" } else { "list" }
                ));
            }
            return 0;
        }
        self.next_temp = base + n;
        // Always update max_reg with `base` — BuildList/BuildTuple always writes
        // to `base` regardless of element count (even empty collections).
        let max_used = if n > 0 { base + n - 1 } else { base };
        if max_used > self.max_reg {
            self.max_reg = max_used;
        }
        for (i, item) in (0u32..).zip(items.iter()) {
            let slot = base + i;
            let saved = self.next_temp;
            let insn_before = self.insns.len();
            let r = self.compile_expr(item);
            if r != slot {
                let single = self.insns.len() == insn_before + 1;
                if single && r >= self.base_temp && self.retarget_last(r, slot) {
                    // retargeted in place — no Move needed
                } else {
                    self.emit(Insn::Move(slot, r));
                }
            }
            self.next_temp = saved;
        }
        if is_tuple {
            self.emit(Insn::BuildTuple(base, base, n));
        } else {
            self.emit(Insn::BuildList(base, base, n));
        }
        self.next_temp = base + 1;
        base
    }

    /// Compile `[a, *b, c]` / `(a, *b, c)` — PEP 448 sequence splat.
    /// Strategy: build an empty list, then for each item emit either
    /// `ListAppend` (literal) or `ListExtend` (splat).  Tuples reuse the same
    /// path then convert via the `tuple` builtin at the end.
    fn compile_unpack_list_or_tuple(&mut self, items: &[Expr], is_tuple: bool) -> Reg {
        let dst = self.alloc_temp();
        let empty_base = self.next_temp;
        self.next_temp = empty_base + 1;
        if empty_base > self.max_reg {
            self.max_reg = empty_base;
        }
        self.emit(Insn::BuildList(dst, empty_base, 0));
        for item in items {
            match item {
                Expr::Starred(inner) => {
                    let saved = self.next_temp;
                    let r = self.compile_expr(inner);
                    self.emit(Insn::ListExtend(dst, r));
                    self.next_temp = saved;
                }
                _ => {
                    let saved = self.next_temp;
                    let r = self.compile_expr(item);
                    self.emit(Insn::ListAppend(dst, r));
                    self.next_temp = saved;
                }
            }
        }
        if !is_tuple {
            self.next_temp = dst + 1;
            return dst;
        }
        // Convert the freshly-built list into a tuple via the `tuple` builtin.
        let frame = self.next_temp;
        if frame.checked_add(2).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("register overflow in tuple splat".to_string());
            }
            return 0;
        }
        self.next_temp = frame + 2;
        if frame + 1 > self.max_reg {
            self.max_reg = frame + 1;
        }
        let tuple_name_idx = self.intern_name("tuple");
        self.emit(Insn::LoadGlobal(frame, tuple_name_idx));
        self.emit(Insn::Move(frame + 1, dst));
        self.emit(Insn::Call(frame, 1));
        self.next_temp = frame + 1;
        frame
    }

    /// Compile `{a, *b, c}` — PEP 448 set splat.  Strategy: build an empty
    /// list (uniform path with non-splat sets), then convert via the `set`
    /// builtin.  Splat elements are appended via `ListExtend`, ordinary
    /// elements via `ListAppend`.
    fn compile_set_literal(&mut self, items: &[Expr]) -> Reg {
        let has_splat = items.iter().any(|e| matches!(e, Expr::Starred(_)));
        if !has_splat {
            // Fast path: no splat — same code shape as the original.
            let n = items.len() as Reg;
            let frame = self.next_temp;
            if frame.checked_add(1 + n).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many elements in set literal".to_string());
                }
                return 0;
            }
            self.next_temp = frame + 1 + n;
            if frame + n > self.max_reg {
                self.max_reg = frame + n;
            }
            let set_name_idx = self.intern_name("set");
            self.emit(Insn::LoadGlobal(frame, set_name_idx));
            let list_r = frame + 1;
            let saved = self.next_temp;
            let list_base = self.next_temp;
            if list_base.checked_add(n).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many elements in set literal".to_string());
                }
                return 0;
            }
            self.next_temp = list_base + n;
            if list_base + n - 1 > self.max_reg {
                self.max_reg = list_base + n - 1;
            }
            for (i, item) in (0u32..).zip(items.iter()) {
                let slot = list_base + i;
                let ns = self.next_temp;
                let r = self.compile_expr(item);
                if r != slot {
                    self.emit(Insn::Move(slot, r));
                }
                self.next_temp = ns;
            }
            self.emit(Insn::BuildList(list_r, list_base, n));
            self.next_temp = saved;
            self.next_temp = frame + 2;
            if frame + 1 > self.max_reg {
                self.max_reg = frame + 1;
            }
            self.emit(Insn::Call(frame, 1));
            self.next_temp = frame + 1;
            return frame;
        }

        // Slow path with splats: build list incrementally, then call set(list).
        let frame = self.next_temp;
        if frame.checked_add(2).is_none() {
            self.failed = true;
            if self.error_msg.is_none() {
                self.error_msg = Some("register overflow in set splat".to_string());
            }
            return 0;
        }
        self.next_temp = frame + 2;
        if frame + 1 > self.max_reg {
            self.max_reg = frame + 1;
        }
        let set_name_idx = self.intern_name("set");
        self.emit(Insn::LoadGlobal(frame, set_name_idx));
        let list_r = frame + 1;
        let empty_base = self.next_temp;
        self.next_temp = empty_base + 1;
        if empty_base > self.max_reg {
            self.max_reg = empty_base;
        }
        self.emit(Insn::BuildList(list_r, empty_base, 0));
        for item in items {
            match item {
                Expr::Starred(inner) => {
                    let saved = self.next_temp;
                    let r = self.compile_expr(inner);
                    self.emit(Insn::ListExtend(list_r, r));
                    self.next_temp = saved;
                }
                _ => {
                    let saved = self.next_temp;
                    let r = self.compile_expr(item);
                    self.emit(Insn::ListAppend(list_r, r));
                    self.next_temp = saved;
                }
            }
        }
        self.emit(Insn::Call(frame, 1));
        self.next_temp = frame + 1;
        frame
    }

    /// Compile `{k1: v1, **m, k2: v2}` — supports PEP 448 dict splat.
    /// Fast path (no `**` splats) uses `BuildDict` with pre-staged key/value
    /// slots, identical to the pre-PEP-448 shape.  Slow path builds an empty
    /// dict and emits `SetItem` for pairs / `DictUpdate` for splats.
    fn dict_key_kind_hint(items: &[DictItem]) -> DictKeyKindHint {
        let mut saw_dynamic = false;
        for item in items {
            let DictItem::Pair(key, _) = item else {
                return DictKeyKindHint::Unknown;
            };
            match key {
                // Both forms always produce an exact string Value.
                Expr::Str(_) | Expr::FString(_) => {}
                // These literal forms are statically exact non-strings. One is
                // enough for `_PyDict_FromItems` to choose a General table.
                Expr::Int(_)
                | Expr::BigInt(_)
                | Expr::Float(_)
                | Expr::Complex(_, _)
                | Expr::Bytes(_)
                | Expr::Bool(_)
                | Expr::None
                | Expr::Ellipsis
                | Expr::List(_)
                | Expr::Tuple(_)
                | Expr::Dict(_)
                | Expr::Set(_) => return DictKeyKindHint::General,
                _ => saw_dynamic = true,
            }
        }
        if saw_dynamic {
            DictKeyKindHint::Unknown
        } else {
            DictKeyKindHint::Unicode
        }
    }

    fn compile_dict_literal(&mut self, items: &[DictItem]) -> Reg {
        let has_splat = items.iter().any(|i| matches!(i, DictItem::DoubleSplat(_)));
        if !has_splat {
            let n = items.len() as Reg;
            let base = self.next_temp;
            let slots_needed = n.saturating_mul(2);
            if base.checked_add(slots_needed).is_none() {
                self.failed = true;
                if self.error_msg.is_none() {
                    self.error_msg = Some("too many entries in dict literal".to_string());
                }
                return 0;
            }
            if base > self.max_reg {
                self.max_reg = base;
            }
            self.next_temp = base + n.saturating_mul(2);
            if self.next_temp > 0 && self.next_temp - 1 > self.max_reg {
                self.max_reg = self.next_temp - 1;
            }
            for (i, item) in (0u32..).zip(items.iter()) {
                let (key_expr, val_expr) = match item {
                    DictItem::Pair(k, v) => (k, v),
                    DictItem::DoubleSplat(_) => unreachable!("has_splat is false"),
                };
                let k_slot = base + i * 2;
                let v_slot = base + i * 2 + 1;
                let saved = self.next_temp;
                let insn_before = self.insns.len();
                let kr = self.compile_expr(key_expr);
                if kr != k_slot {
                    let single = self.insns.len() == insn_before + 1;
                    if single && kr >= self.base_temp && self.retarget_last(kr, k_slot) {
                        // retargeted in place — no Move needed
                    } else {
                        self.emit(Insn::Move(k_slot, kr));
                    }
                }
                self.next_temp = saved;
                let insn_before = self.insns.len();
                let vr = self.compile_expr(val_expr);
                if vr != v_slot {
                    let single = self.insns.len() == insn_before + 1;
                    if single && vr >= self.base_temp && self.retarget_last(vr, v_slot) {
                        // retargeted in place — no Move needed
                    } else {
                        self.emit(Insn::Move(v_slot, vr));
                    }
                }
                self.next_temp = saved;
            }
            self.emit(Insn::BuildDict(
                base,
                base,
                n,
                Self::dict_key_kind_hint(items),
            ));
            self.next_temp = base + 1;
            return base;
        }

        // Slow path: build empty dict, populate via SetItem / DictUpdate.
        let dst = self.alloc_temp();
        let empty_base = self.next_temp;
        self.next_temp = empty_base + 1;
        if empty_base > self.max_reg {
            self.max_reg = empty_base;
        }
        self.emit(Insn::BuildDict(
            dst,
            empty_base,
            0,
            DictKeyKindHint::Unicode,
        ));
        for item in items {
            match item {
                DictItem::Pair(k, v) => {
                    let saved = self.next_temp;
                    let kr = self.compile_expr(k);
                    let vr = self.compile_expr(v);
                    self.emit(Insn::SetItem(dst, kr, vr));
                    self.next_temp = saved;
                }
                DictItem::DoubleSplat(e) => {
                    let saved = self.next_temp;
                    let r = self.compile_expr(e);
                    self.emit(Insn::DictUpdate(dst, r));
                    self.next_temp = saved;
                }
            }
        }
        self.next_temp = dst + 1;
        dst
    }
}
