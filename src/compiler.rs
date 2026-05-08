use std::collections::HashMap;

use crate::ast::{AssignTarget, BinaryOp, CmpOp, Expr, Stmt};
use crate::bytecode::{FnCode, Insn, Reg};
use crate::value::{UserFunction, Value};

/// Compile a user function to bytecode.
/// Returns None if the function uses features the VM does not support.
pub fn compile_fn(func: &UserFunction) -> Option<FnCode> {
    if func.params.iter().any(|p| p.is_args || p.is_kwargs) {
        return None;
    }
    if !func.nonlocal_names.is_empty() || !func.global_names.is_empty() {
        return None;
    }
    if has_unsupported(func.body.as_slice()) {
        return None;
    }
    let mut c = Compiler::new(func);
    c.compile_block(&func.body.clone());
    c.finish()
}

fn has_unsupported(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| stmt_unsupported(s))
}

fn stmt_unsupported(s: &Stmt) -> bool {
    match s {
        Stmt::Try { .. }
        | Stmt::With { .. }
        | Stmt::Class { .. }
        | Stmt::Raise { .. }
        | Stmt::Import { .. }
        | Stmt::ImportFrom { .. }
        | Stmt::Delete(_)
        | Stmt::Global(_)
        | Stmt::Nonlocal(_)
        | Stmt::Def { .. } => true,
        Stmt::If {
            branches,
            else_branch,
        } => {
            branches.iter().any(|(_, b)| has_unsupported(b))
                || else_branch.as_deref().map_or(false, has_unsupported)
        }
        Stmt::While {
            body, else_branch, ..
        } => has_unsupported(body) || else_branch.as_deref().map_or(false, has_unsupported),
        Stmt::For {
            body, else_branch, ..
        } => has_unsupported(body) || else_branch.as_deref().map_or(false, has_unsupported),
        _ => false,
    }
}

fn cmp_to_binary(op: CmpOp) -> BinaryOp {
    match op {
        CmpOp::Eq => BinaryOp::Eq,
        CmpOp::Ne => BinaryOp::Ne,
        CmpOp::Lt => BinaryOp::Lt,
        CmpOp::Le => BinaryOp::Le,
        CmpOp::Gt => BinaryOp::Gt,
        CmpOp::Ge => BinaryOp::Ge,
        CmpOp::In => BinaryOp::In,
        CmpOp::NotIn => BinaryOp::NotIn,
        CmpOp::Is => BinaryOp::Is,
        CmpOp::IsNot => BinaryOp::IsNot,
    }
}

struct LoopCtx {
    break_patches: Vec<usize>,
    continue_target: usize,
}

struct Compiler<'a> {
    func: &'a UserFunction,
    insns: Vec<Insn>,
    consts: Vec<Value>,
    names: Vec<String>,
    name_map: HashMap<String, u16>,
    next_temp: Reg,
    base_temp: Reg,
    iter_depth: u8,
    max_iter: u8,
    max_reg: Reg,
    loops: Vec<LoopCtx>,
    failed: bool,
}

impl<'a> Compiler<'a> {
    fn new(func: &'a UserFunction) -> Self {
        let n = func.local_index.len();
        let base_temp = if n > 255 {
            return Self::failed_compiler(func);
        } else {
            n as Reg
        };
        Self {
            func,
            insns: Vec::new(),
            consts: Vec::new(),
            names: Vec::new(),
            name_map: HashMap::new(),
            next_temp: base_temp,
            base_temp,
            iter_depth: 0,
            max_iter: 0,
            max_reg: if base_temp > 0 { base_temp - 1 } else { 0 },
            loops: Vec::new(),
            failed: false,
        }
    }

    fn failed_compiler(func: &'a UserFunction) -> Self {
        let c = Self {
            func,
            insns: Vec::new(),
            consts: Vec::new(),
            names: Vec::new(),
            name_map: HashMap::new(),
            next_temp: 0,
            base_temp: 0,
            iter_depth: 0,
            max_iter: 0,
            max_reg: 0,
            loops: Vec::new(),
            failed: true,
        };
        c
    }

    fn intern_name(&mut self, name: &str) -> u16 {
        if let Some(&idx) = self.name_map.get(name) {
            return idx;
        }
        let idx = self.names.len() as u16;
        self.names.push(name.to_string());
        self.name_map.insert(name.to_string(), idx);
        idx
    }

    fn intern_const(&mut self, val: Value) -> u16 {
        for (i, v) in self.consts.iter().enumerate() {
            if const_eq(v, &val) {
                return i as u16;
            }
        }
        let idx = self.consts.len() as u16;
        self.consts.push(val);
        idx
    }

    fn alloc_temp(&mut self) -> Reg {
        let r = self.next_temp;
        if r == Reg::MAX {
            self.failed = true;
            return 0;
        }
        self.next_temp += 1;
        if r > self.max_reg {
            self.max_reg = r;
        }
        r
    }

    fn free_temp(&mut self, r: Reg) {
        if r >= self.base_temp && self.next_temp > 0 && r + 1 == self.next_temp {
            self.next_temp -= 1;
        }
    }

    fn alloc_iter(&mut self) -> u8 {
        let s = self.iter_depth;
        self.iter_depth += 1;
        if self.iter_depth > self.max_iter {
            self.max_iter = self.iter_depth;
        }
        s
    }

    fn free_iter(&mut self) {
        if self.iter_depth > 0 {
            self.iter_depth -= 1;
        }
    }

    fn emit(&mut self, insn: Insn) -> usize {
        let idx = self.insns.len();
        self.insns.push(insn);
        idx
    }

    fn pc(&self) -> usize {
        self.insns.len()
    }

    fn patch_jump(&mut self, idx: usize) {
        let target = self.insns.len() as i32;
        let after_jump = idx as i32 + 1;
        let offset = target - after_jump;
        match &mut self.insns[idx] {
            Insn::Jump(off)
            | Insn::JumpIfFalse(_, off)
            | Insn::JumpIfTrue(_, off)
            | Insn::ForIter(_, _, off) => *off = offset,
            _ => self.failed = true,
        }
    }

    fn local_reg(&self, name: &str) -> Option<Reg> {
        self.func.local_index.get(name).copied().map(|i| i as Reg)
    }

    fn compile_block(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            if self.failed {
                return;
            }
            self.compile_stmt(stmt);
        }
    }

    fn compile_stmt(&mut self, stmt: &Stmt) {
        if self.failed {
            return;
        }
        match stmt {
            Stmt::Pass => {}
            Stmt::Break => {
                if self.loops.is_empty() {
                    self.failed = true;
                    return;
                }
                let idx = self.emit(Insn::Jump(0));
                let last = self.loops.len() - 1;
                self.loops[last].break_patches.push(idx);
            }
            Stmt::Continue => {
                if self.loops.is_empty() {
                    self.failed = true;
                    return;
                }
                let last = self.loops.len() - 1;
                let target = self.loops[last].continue_target;
                let idx = self.emit(Insn::Jump(0));
                let from = idx as i32 + 1;
                let offset = target as i32 - from;
                if let Insn::Jump(off) = &mut self.insns[idx] {
                    *off = offset;
                }
            }
            Stmt::Return(None) => {
                self.emit(Insn::ReturnNone);
            }
            Stmt::Return(Some(expr)) => {
                let r = self.compile_expr(expr);
                self.emit(Insn::Return(r));
                self.free_temp(r);
            }
            Stmt::Expr(expr) => {
                let r = self.compile_expr(expr);
                self.free_temp(r);
            }
            Stmt::Assign(target, expr) => {
                self.compile_assign(target, expr);
            }
            Stmt::AugAssign { target, op, expr } => {
                self.compile_aug_assign(target, *op, expr);
            }
            Stmt::AttrAssign { target, name, expr } => {
                let obj = self.compile_expr(target);
                let val = self.compile_expr(expr);
                let name_idx = self.intern_name(name);
                self.emit(Insn::SetAttr(obj, name_idx, val));
                self.free_temp(val);
                self.free_temp(obj);
            }
            Stmt::IndexAssign {
                target,
                index,
                expr,
            } => {
                let obj = self.compile_expr(target);
                let idx = self.compile_expr(index);
                let val = self.compile_expr(expr);
                self.emit(Insn::SetItem(obj, idx, val));
                self.free_temp(val);
                self.free_temp(idx);
                self.free_temp(obj);
            }
            Stmt::Assert { test, msg: _ } => {
                // Compile the test; if truthy skip the error path.
                // For false assertions we fall back by marking failed — the tree-walker
                // will handle the actual AssertionError message.
                let r = self.compile_expr(test);
                let jmp = self.emit(Insn::JumpIfTrue(r, 0));
                self.free_temp(r);
                // Emit a failing sentinel: use an impossible Call that the VM will never
                // reach under correct behavior. We mark failed here so this function
                // body exits compile-time — the VM won't handle assert failures.
                self.failed = true;
                let _ = jmp;
            }
            Stmt::If {
                branches,
                else_branch,
            } => {
                self.compile_if(branches, else_branch.as_deref());
            }
            Stmt::While {
                cond,
                body,
                else_branch,
            } => {
                self.compile_while(cond, body, else_branch.as_deref());
            }
            Stmt::For {
                target,
                iter,
                body,
                else_branch,
            } => {
                self.compile_for(target, iter, body, else_branch.as_deref());
            }
            _ => {
                self.failed = true;
            }
        }
    }

    fn compile_assign(&mut self, target: &AssignTarget, expr: &Expr) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    self.compile_expr_into(expr, reg);
                } else {
                    self.failed = true;
                }
            }
            AssignTarget::Tuple(targets) => {
                let src = self.compile_expr(expr);
                let n = targets.len() as u8;
                if n == 0 {
                    self.free_temp(src);
                    return;
                }
                let base = self.next_temp;
                // Reserve unpack slots
                if base as usize + n as usize > 256 {
                    self.failed = true;
                    return;
                }
                self.next_temp = base + n;
                if self.next_temp - 1 > self.max_reg {
                    self.max_reg = self.next_temp - 1;
                }
                self.emit(Insn::Unpack(base, src, n));
                self.free_temp(src);
                for (i, t) in targets.iter().enumerate() {
                    match t {
                        AssignTarget::Name(name) => {
                            if let Some(reg) = self.local_reg(name) {
                                self.emit(Insn::Move(reg, base + i as u8));
                            } else {
                                self.failed = true;
                                return;
                            }
                        }
                        _ => {
                            self.failed = true;
                            return;
                        }
                    }
                }
                self.next_temp = base;
            }
            AssignTarget::Attr(obj_expr, attr) => {
                let obj = self.compile_expr(obj_expr);
                let val = self.compile_expr(expr);
                let name_idx = self.intern_name(attr);
                self.emit(Insn::SetAttr(obj, name_idx, val));
                self.free_temp(val);
                self.free_temp(obj);
            }
            AssignTarget::Index(obj_expr, idx_expr) => {
                let obj = self.compile_expr(obj_expr);
                let idx = self.compile_expr(idx_expr);
                let val = self.compile_expr(expr);
                self.emit(Insn::SetItem(obj, idx, val));
                self.free_temp(val);
                self.free_temp(idx);
                self.free_temp(obj);
            }
        }
    }

    fn compile_aug_assign(&mut self, target: &AssignTarget, op: BinaryOp, expr: &Expr) {
        match target {
            AssignTarget::Name(name) => {
                if let Some(reg) = self.local_reg(name) {
                    let rhs = self.compile_expr(expr);
                    self.emit(Insn::BinOp(reg, reg, op, rhs));
                    self.free_temp(rhs);
                } else {
                    self.failed = true;
                }
            }
            _ => {
                self.failed = true;
            }
        }
    }

    fn compile_if(&mut self, branches: &[(Expr, Vec<Stmt>)], else_branch: Option<&[Stmt]>) {
        let has_else = else_branch.is_some();
        let n = branches.len();
        let mut end_patches: Vec<usize> = Vec::new();

        for (bi, (cond, body)) in branches.iter().enumerate() {
            let cond_reg = self.compile_expr(cond);
            let jmp_false = self.emit(Insn::JumpIfFalse(cond_reg, 0));
            self.free_temp(cond_reg);

            self.compile_block(body);
            if self.failed {
                return;
            }

            if bi < n - 1 || has_else {
                let jmp_end = self.emit(Insn::Jump(0));
                end_patches.push(jmp_end);
            }
            self.patch_jump(jmp_false);
        }

        if let Some(else_stmts) = else_branch {
            self.compile_block(else_stmts);
        }

        for idx in end_patches {
            self.patch_jump(idx);
        }
    }

    fn compile_while(&mut self, cond: &Expr, body: &[Stmt], else_branch: Option<&[Stmt]>) {
        let loop_start = self.pc();

        self.loops.push(LoopCtx {
            break_patches: Vec::new(),
            continue_target: loop_start,
        });

        let cond_reg = self.compile_expr(cond);
        let exit_jmp = self.emit(Insn::JumpIfFalse(cond_reg, 0));
        self.free_temp(cond_reg);

        self.compile_block(body);
        if self.failed {
            return;
        }

        let back_from = self.pc() as i32 + 1;
        let back_offset = loop_start as i32 - back_from;
        self.emit(Insn::Jump(back_offset));

        self.patch_jump(exit_jmp);

        let ctx = self.loops.pop().unwrap();
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }

        if let Some(else_stmts) = else_branch {
            self.compile_block(else_stmts);
        }
    }

    fn compile_for(
        &mut self,
        target: &AssignTarget,
        iter_expr: &Expr,
        body: &[Stmt],
        else_branch: Option<&[Stmt]>,
    ) {
        let iter_slot = self.alloc_iter();

        let src = self.compile_expr(iter_expr);
        self.emit(Insn::GetIter(iter_slot, src));
        self.free_temp(src);

        let loop_start = self.pc();
        let item_reg = self.alloc_temp();
        let exit_jmp = self.emit(Insn::ForIter(item_reg, iter_slot, 0));

        // Assign item to loop target
        match target {
            AssignTarget::Name(name) => {
                if let Some(var_reg) = self.local_reg(name) {
                    if item_reg != var_reg {
                        self.emit(Insn::Move(var_reg, item_reg));
                    }
                    self.free_temp(item_reg);
                } else {
                    self.failed = true;
                    return;
                }
            }
            AssignTarget::Tuple(targets) => {
                let n = targets.len() as u8;
                let base = self.next_temp;
                if base as usize + n as usize > 256 {
                    self.failed = true;
                    return;
                }
                self.next_temp = base + n;
                if self.next_temp - 1 > self.max_reg {
                    self.max_reg = self.next_temp - 1;
                }
                self.emit(Insn::Unpack(base, item_reg, n));
                self.free_temp(item_reg);
                for (i, t) in targets.iter().enumerate() {
                    match t {
                        AssignTarget::Name(name) => {
                            if let Some(reg) = self.local_reg(name) {
                                self.emit(Insn::Move(reg, base + i as u8));
                            } else {
                                self.failed = true;
                                return;
                            }
                        }
                        _ => {
                            self.failed = true;
                            return;
                        }
                    }
                }
                self.next_temp = base;
            }
            _ => {
                self.failed = true;
                return;
            }
        }

        self.loops.push(LoopCtx {
            break_patches: Vec::new(),
            continue_target: loop_start,
        });

        self.compile_block(body);
        if self.failed {
            return;
        }

        let back_from = self.pc() as i32 + 1;
        let back_offset = loop_start as i32 - back_from;
        self.emit(Insn::Jump(back_offset));

        self.patch_jump(exit_jmp);

        let ctx = self.loops.pop().unwrap();
        for idx in ctx.break_patches {
            self.patch_jump(idx);
        }

        self.free_iter();

        if let Some(else_stmts) = else_branch {
            self.compile_block(else_stmts);
        }
    }

    fn compile_expr_into(&mut self, expr: &Expr, dst: Reg) {
        if self.failed {
            return;
        }
        let saved_next = self.next_temp;
        let r = self.compile_expr(expr);
        if r != dst {
            self.emit(Insn::Move(dst, r));
            if r >= self.base_temp {
                self.next_temp = saved_next;
            }
        }
    }

    fn compile_expr(&mut self, expr: &Expr) -> Reg {
        if self.failed {
            return 0;
        }
        match expr {
            Expr::None => {
                let dst = self.alloc_temp();
                self.emit(Insn::LoadNone(dst));
                dst
            }
            Expr::Int(v) => {
                let idx = self.intern_const(Value::Int(*v));
                let dst = self.alloc_temp();
                self.emit(Insn::LoadConst(dst, idx));
                dst
            }
            Expr::Float(v) => {
                let idx = self.intern_const(Value::Float(*v));
                let dst = self.alloc_temp();
                self.emit(Insn::LoadConst(dst, idx));
                dst
            }
            Expr::Str(s) => {
                let idx = self.intern_const(Value::Str(s.clone()));
                let dst = self.alloc_temp();
                self.emit(Insn::LoadConst(dst, idx));
                dst
            }
            Expr::Bool(b) => {
                let idx = self.intern_const(Value::Bool(*b));
                let dst = self.alloc_temp();
                self.emit(Insn::LoadConst(dst, idx));
                dst
            }
            Expr::Var(name) => {
                if let Some(reg) = self.local_reg(name) {
                    let name_idx = self.intern_name(name);
                    self.emit(Insn::CheckLocal(reg, name_idx));
                    reg
                } else {
                    let name_idx = self.intern_name(name);
                    let dst = self.alloc_temp();
                    self.emit(Insn::LoadGlobal(dst, name_idx));
                    dst
                }
            }
            Expr::Unary { op, expr } => {
                let src = self.compile_expr(expr);
                let dst = self.ensure_dst(src);
                self.emit(Insn::UnaryOp(dst, *op, src));
                dst
            }
            Expr::Binary { left, op, right } => match op {
                BinaryOp::And => {
                    let lhs = self.compile_expr(left);
                    let dst = self.ensure_dst(lhs);
                    if dst != lhs {
                        self.emit(Insn::Move(dst, lhs));
                    }
                    let jmp = self.emit(Insn::JumpIfFalse(dst, 0));
                    let saved = self.next_temp;
                    let rhs = self.compile_expr(right);
                    self.emit(Insn::Move(dst, rhs));
                    self.next_temp = saved;
                    self.patch_jump(jmp);
                    dst
                }
                BinaryOp::Or => {
                    let lhs = self.compile_expr(left);
                    let dst = self.ensure_dst(lhs);
                    if dst != lhs {
                        self.emit(Insn::Move(dst, lhs));
                    }
                    let jmp = self.emit(Insn::JumpIfTrue(dst, 0));
                    let saved = self.next_temp;
                    let rhs = self.compile_expr(right);
                    self.emit(Insn::Move(dst, rhs));
                    self.next_temp = saved;
                    self.patch_jump(jmp);
                    dst
                }
                _ => {
                    let lhs = self.compile_expr(left);
                    let rhs = self.compile_expr(right);
                    let dst = self.ensure_dst(lhs);
                    self.emit(Insn::BinOp(dst, lhs, *op, rhs));
                    self.free_temp(rhs);
                    dst
                }
            },
            Expr::Compare { left, ops } => {
                if ops.len() == 1 {
                    let (cmp_op, right) = &ops[0];
                    let lhs = self.compile_expr(left);
                    let rhs = self.compile_expr(right);
                    let bin_op = cmp_to_binary(*cmp_op);
                    let dst = self.ensure_dst(lhs);
                    self.emit(Insn::BinOp(dst, lhs, bin_op, rhs));
                    self.free_temp(rhs);
                    dst
                } else {
                    self.failed = true;
                    0
                }
            }
            Expr::Call { func, args } => {
                if args
                    .iter()
                    .any(|a| a.splat || a.double_splat || a.name.is_some())
                {
                    self.failed = true;
                    return 0;
                }
                let argc = args.len() as u8;
                let func_reg = self.next_temp;
                let frame_top = func_reg.wrapping_add(1).wrapping_add(argc);
                if frame_top < func_reg {
                    self.failed = true;
                    return 0;
                }
                // Reserve func + arg slots
                self.next_temp = frame_top;
                if frame_top > 0 && frame_top - 1 > self.max_reg {
                    self.max_reg = frame_top - 1;
                }

                // Compile function expression into func_reg
                let saved = self.next_temp;
                self.compile_expr_into(func, func_reg);
                self.next_temp = saved;

                // Compile each argument into its slot
                for (i, arg) in args.iter().enumerate() {
                    let arg_reg = func_reg + 1 + i as u8;
                    let saved = self.next_temp;
                    let r = self.compile_expr(&arg.value);
                    if r != arg_reg {
                        self.emit(Insn::Move(arg_reg, r));
                    }
                    self.next_temp = saved;
                }

                self.emit(Insn::Call(func_reg, func_reg, argc));
                self.next_temp = func_reg + 1;
                func_reg
            }
            Expr::Attr { target, name } => {
                let obj = self.compile_expr(target);
                let name_idx = self.intern_name(name);
                let dst = self.ensure_dst(obj);
                self.emit(Insn::GetAttr(dst, obj, name_idx));
                dst
            }
            Expr::Index { target, index } => {
                let obj = self.compile_expr(target);
                let idx = self.compile_expr(index);
                let dst = self.ensure_dst(obj);
                self.emit(Insn::GetItem(dst, obj, idx));
                self.free_temp(idx);
                dst
            }
            Expr::List(items) => self.compile_collection(items, false),
            Expr::Tuple(items) => self.compile_collection(items, true),
            Expr::Dict(pairs) => {
                // pairs: Vec<(Expr, Expr)> — interleave keys and values
                let n = pairs.len() as u8;
                let base = self.next_temp;
                let slots_needed = (n as usize).saturating_mul(2);
                if base as usize + slots_needed > 256 {
                    self.failed = true;
                    return 0;
                }
                // Reserve all slots
                self.next_temp = base + n.saturating_mul(2);
                if self.next_temp > 0 && self.next_temp - 1 > self.max_reg {
                    self.max_reg = self.next_temp - 1;
                }
                for (i, (key_expr, val_expr)) in pairs.iter().enumerate() {
                    let k_slot = base + (i * 2) as u8;
                    let v_slot = base + (i * 2 + 1) as u8;
                    let saved = self.next_temp;
                    let kr = self.compile_expr(key_expr);
                    if kr != k_slot {
                        self.emit(Insn::Move(k_slot, kr));
                    }
                    self.next_temp = saved;
                    let saved = self.next_temp;
                    let vr = self.compile_expr(val_expr);
                    if vr != v_slot {
                        self.emit(Insn::Move(v_slot, vr));
                    }
                    self.next_temp = saved;
                }
                self.emit(Insn::BuildDict(base, base, n));
                self.next_temp = base + 1;
                base
            }
            Expr::Ternary { cond, then, else_ } => {
                let cond_reg = self.compile_expr(cond);
                let jmp_false = self.emit(Insn::JumpIfFalse(cond_reg, 0));
                self.free_temp(cond_reg);

                let dst = self.alloc_temp();
                let saved = self.next_temp;
                self.compile_expr_into(then, dst);
                self.next_temp = saved;

                let jmp_end = self.emit(Insn::Jump(0));
                self.patch_jump(jmp_false);

                let saved = self.next_temp;
                self.compile_expr_into(else_, dst);
                self.next_temp = saved;

                self.patch_jump(jmp_end);
                dst
            }
            _ => {
                self.failed = true;
                0
            }
        }
    }

    fn compile_collection(&mut self, items: &[Expr], is_tuple: bool) -> Reg {
        let n = items.len() as u8;
        let base = self.next_temp;
        if base as usize + n as usize > 256 {
            self.failed = true;
            return 0;
        }
        // Reserve all item slots upfront
        self.next_temp = base + n;
        if n > 0 && base + n - 1 > self.max_reg {
            self.max_reg = base + n - 1;
        }
        // Compile each item into its slot
        for (i, item) in items.iter().enumerate() {
            let slot = base + i as u8;
            let saved = self.next_temp;
            let r = self.compile_expr(item);
            if r != slot {
                self.emit(Insn::Move(slot, r));
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

    /// Allocate a result register: reuse `candidate` if it's a temp, otherwise fresh temp.
    fn ensure_dst(&mut self, candidate: Reg) -> Reg {
        if candidate >= self.base_temp {
            candidate
        } else {
            self.alloc_temp()
        }
    }

    fn finish(self) -> Option<FnCode> {
        if self.failed {
            return None;
        }
        let num_regs = if self.max_reg >= self.base_temp || self.base_temp == 0 {
            self.max_reg.saturating_add(1)
        } else {
            self.base_temp
        };
        Some(FnCode {
            insns: self.insns,
            consts: self.consts,
            names: self.names,
            num_regs,
            num_iters: self.max_iter,
            num_locals: self.base_temp,
        })
    }
}

fn const_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => x.to_bits() == y.to_bits(),
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::None, Value::None) => true,
        _ => false,
    }
}
